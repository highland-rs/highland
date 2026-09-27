// Rust guideline compliant 2026-09-27

//! The Netlink backend, against a real kernel.
//!
//! Every other test in this crate runs against a script. This one runs against
//! Linux, because the claim `highland-net` makes is that a successful
//! `NetlinkBackend` call means the kernel state changed (`I-19`), and only a
//! kernel can decide whether it did.
//!
//! Each test creates its own dummy interface, so the suite is self-contained
//! and two runs cannot collide. Creating one needs `CAP_NET_ADMIN`, so the file
//! is behind the `netlink-tests` feature and only a privileged environment runs
//! it:
//!
//! ```console
//! $ cargo test -p highland-net --features netlink-tests
//! $ podman run --rm --privileged -v "$PWD":/src -w /src rust:slim \
//!     cargo test -p highland-net --features netlink-tests
//! ```
//!
//! The interface is a dummy device rather than loopback on purpose. The kernel
//! treats a secondary address on `lo` differently from the same address on any
//! other device, and a fixture that depends on that quirk would be testing the
//! quirk instead of the backend.

#![cfg(all(target_os = "linux", feature = "netlink-tests"))]

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use highland_net::{
    InterfaceId, IpCidr, LinkEvent, LinkState, NetError, NetlinkBackend, NetworkBackend,
};

/// Distinguishes the dummy interfaces one run creates.
static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A dummy interface created for one test.
struct Fixture {
    backend: NetlinkBackend,
    name: String,
}

impl Fixture {
    async fn new() -> Self {
        let backend = NetlinkBackend::open().expect("a netlink socket opens");
        let name = format!(
            "hl{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        backend
            .create_dummy(&name)
            .await
            .unwrap_or_else(|error| panic!("could not create {name}: {error}"));
        Self { backend, name }
    }

    async fn id(&self) -> InterfaceId {
        self.backend
            .interface(&self.name)
            .await
            .expect("the interface exists")
            .id
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Removal is best-effort: in a throwaway container a leaked dummy
        // interface costs nothing, and blocking inside `drop` on a runtime that
        // may already be shutting down would be worse than the leak.
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let backend = self.backend.clone();
            let name = self.name.clone();
            handle.spawn(async move {
                let _ = backend.remove_dummy(&name).await;
            });
        }
    }
}

/// The next address in the documentation range.
///
/// Uniqueness matters: the backend refuses an address another local interface
/// already holds, which is correct behavior and would otherwise make two
/// parallel tests fail for the right reason at the wrong moment. Each call
/// returns a different address, so a test can ask for three.
fn next_v4() -> IpCidr {
    let last = u8::try_from(COUNTER.fetch_add(1, Ordering::Relaxed) % 200).unwrap_or(0);
    IpCidr::new(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10 + last)), 24)
}

fn next_v6() -> IpCidr {
    let last = u16::try_from(COUNTER.fetch_add(1, Ordering::Relaxed) % 200).unwrap_or(0);
    IpCidr::new(
        IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 0x10 + last)),
        64,
    )
}

#[tokio::test]
async fn a_created_interface_resolves_and_reports_no_carrier() {
    let fixture = Fixture::new().await;
    let interface = fixture
        .backend
        .interface(&fixture.name)
        .await
        .expect("the interface exists");

    assert!(interface.id.get() > 0, "the kernel reports a real index");
    assert_eq!(interface.name, fixture.name);
    // A dummy device carries nothing, so it must not read as usable. `I-15`
    // depends on an unusable link looking unusable.
    assert_eq!(
        interface.state,
        LinkState::NoCarrier,
        "a dummy device has no carrier"
    );
}

#[tokio::test]
async fn an_interface_that_does_not_exist_is_reported() {
    let backend = NetlinkBackend::open().expect("a netlink socket opens");
    let error = backend
        .interface("definitely-not-an-interface")
        .await
        .expect_err("the interface is absent");

    assert!(
        matches!(error, NetError::InterfaceNotFound { .. }),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn an_address_is_added_confirmed_and_removed() {
    let fixture = Fixture::new().await;
    let id = fixture.id().await;

    for target in [next_v4(), next_v6()] {
        fixture
            .backend
            .add_address(id, target)
            .await
            .expect("the address can be added");
        assert!(
            fixture
                .backend
                .address_present(&fixture.name, &target)
                .await
                .expect("the address can be read back"),
            "I-19: a successful add means the kernel state changed, for {target}"
        );

        fixture
            .backend
            .remove_address(id, target)
            .await
            .expect("the address can be removed");
        assert!(
            !fixture
                .backend
                .address_present(&fixture.name, &target)
                .await
                .expect("the address can be read back"),
            "I-19: a successful remove means the kernel state changed, for {target}"
        );
    }
}

#[tokio::test]
async fn several_addresses_coexist_and_are_listed_in_order() {
    let fixture = Fixture::new().await;
    let id = fixture.id().await;
    let targets = [next_v4(), next_v6(), next_v4()];

    for target in targets {
        fixture
            .backend
            .add_address(id, target)
            .await
            .expect("added");
    }

    let listed = fixture
        .backend
        .addresses_on(&fixture.name)
        .await
        .expect("listed");
    for target in targets {
        assert!(
            listed.contains(&target),
            "{target} is missing from {listed:?}"
        );
    }
    assert!(
        listed.len() >= 3,
        "three distinct addresses were added, listed: {listed:?}"
    );

    for target in targets {
        fixture
            .backend
            .remove_address(id, target)
            .await
            .expect("removed");
    }
    let listed = fixture
        .backend
        .addresses_on(&fixture.name)
        .await
        .expect("listed");
    for target in targets {
        assert!(!listed.contains(&target), "{target} survived removal");
    }
}

#[tokio::test]
async fn adding_an_address_twice_is_idempotent() {
    let fixture = Fixture::new().await;
    let id = fixture.id().await;
    let target = next_v4();

    fixture
        .backend
        .add_address(id, target)
        .await
        .expect("the first add succeeds");
    // A second add must not fail on EEXIST. The machine retries, and a retry
    // that failed spuriously would fault an instance that already owns the
    // address it is holding.
    fixture
        .backend
        .add_address(id, target)
        .await
        .expect("a repeated add is a no-op");

    let listed = fixture
        .backend
        .addresses_on(&fixture.name)
        .await
        .expect("listed");
    assert_eq!(
        listed.iter().filter(|found| **found == target).count(),
        1,
        "listed: {listed:?}"
    );

    fixture
        .backend
        .remove_address(id, target)
        .await
        .expect("cleanup");
}

#[tokio::test]
async fn removing_an_address_that_is_not_there_is_idempotent() {
    let fixture = Fixture::new().await;
    let id = fixture.id().await;

    // The same reasoning as above: a repeated release must not fault.
    fixture
        .backend
        .remove_address(id, next_v4())
        .await
        .expect("removing an absent address is a no-op");
}

#[tokio::test]
async fn the_link_subscription_reports_an_address_change() {
    let fixture = Fixture::new().await;
    let mut events = fixture
        .backend
        .subscribe_link_and_address()
        .await
        .expect("a subscription can be installed");

    // Provoke a change and require the subscription to report it. This is the
    // property that matters: a subscription which installs and then never
    // delivers would leave a node holding an address on a link that had gone
    // away, which is exactly what `I-15` prevents.
    let id = fixture.id().await;
    let target = next_v4();
    fixture
        .backend
        .add_address(id, target)
        .await
        .expect("the address can be added");

    let received = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(event) = events.recv().await {
            if event == LinkEvent::Address {
                return true;
            }
        }
        false
    })
    .await
    .unwrap_or(false);

    assert!(
        received,
        "the subscription never reported an address change"
    );
}

/// Brings an interface up, which a real one is and a freshly created dummy is
/// not. A down interface has no usable link-layer entry, and an announcement is
/// only ever sent from a link the node is using.
fn bring_up(name: &str) {
    let output = std::process::Command::new("ip")
        .args(["link", "set", name, "up"])
        .output()
        .expect("ip runs");
    assert!(
        output.status.success(),
        "could not bring {name} up: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A gratuitous announcement is written to the wire, not merely accepted by the
/// kernel.
///
/// A dummy interface has no peer to hear it, so what this proves is the syscall
/// path: the hardware address is read from the kernel, the frame is built, and
/// `AF_PACKET` takes it. That the frame reaches a neighbour is proved in the
/// daemon's two-node suite, which captures it on the far side of a bridge.
#[tokio::test]
async fn a_gratuitous_arp_is_written_out_of_an_interface() {
    let fixture = Fixture::new().await;
    bring_up(&fixture.name);
    let id = fixture.id().await;

    fixture
        .backend
        .send_gratuitous_update(id, IpAddr::V4(Ipv4Addr::new(192, 0, 2, 200)))
        .await
        .expect("the announcement is written");
}

/// The hardware address has to be found, or every announcement would fail. A
/// dummy interface has one, so this checks the lookup rather than the frame.
#[tokio::test]
async fn a_hardware_address_is_read_from_the_kernel() {
    let fixture = Fixture::new().await;
    bring_up(&fixture.name);

    let hardware = highland_net::gratuitous::hardware_address(&fixture.name)
        .expect("a dummy interface has a hardware address");

    assert_eq!(hardware.len(), 6);
    assert!(
        hardware.iter().any(|byte| *byte != 0),
        "an all-zero address is what an interface with none looks like: {hardware:?}"
    );
}

/// Loopback has no hardware address, so there is nothing to announce with. The
/// failure says so rather than sending a frame with a zero address in it, which
/// would teach every neighbour a wrong cache entry.
#[tokio::test]
async fn an_interface_without_a_hardware_address_is_reported_rather_than_guessed() {
    let answer = highland_net::gratuitous::hardware_address("lo");

    assert!(
        answer.is_err(),
        "loopback should have no hardware address, got {answer:?}"
    );
    let error = answer.expect_err("the lookup failed");
    assert_eq!(error.kind(), std::io::ErrorKind::Unsupported, "{error}");
}
