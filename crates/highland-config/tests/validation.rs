// Rust guideline compliant 2026-09-27

//! One test per configuration validation rule in `SPEC.md` §10.4.
//!
//! Every test names the rule it covers, so that a rule without a test is
//! visible: `spec_rule_coverage` at the end of this file asserts that each
//! implemented rule appears at least once.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::net::IpAddr;

use highland_config::{
    Config, InterfaceProbe, KnownInterfaces, ValidationContext, load_and_validate, parse, validate,
};

/// A minimal valid document, used as the base for each negative test.
const VALID: &str = r#"
schema_version = 1

[node]
name = "node-a"

[[instance]]
name = "api"
interface = "eth0"
vrid = 42
priority = 150
advertisement_interval = "1s"

[instance.network]
mode = "unicast"
peers = ["192.0.2.11"]

[[instance.vip]]
address = "192.0.2.10/24"
"#;

fn document(instance_body: &str) -> String {
    format!("{VALID}\n{instance_body}")
}

fn rules_for(text: &str, context: &ValidationContext<'_>) -> Vec<&'static str> {
    let config = parse(text).expect("the document parses");
    match validate(&config, context) {
        Ok(()) => Vec::new(),
        Err(violations) => violations.iter().map(|violation| violation.rule).collect(),
    }
}

fn assert_rule(text: &str, rule: &str) {
    let rules = rules_for(text, &ValidationContext::permissive());
    assert!(rules.contains(&rule), "expected {rule}, got {rules:?}");
}

fn assert_no_rules(text: &str) {
    let rules = rules_for(text, &ValidationContext::permissive());
    assert!(rules.is_empty(), "expected no violations, got {rules:?}");
}

#[test]
fn the_reference_document_is_valid() {
    assert_no_rules(VALID);
}

#[test]
fn v01_rejects_a_vrid_of_zero() {
    assert_rule(
        &document(
            "[[instance]]\nname = \"other\"\ninterface = \"eth1\"\nvrid = 0\nadvertisement_interval = \"1s\"\n[instance.network]\nmode = \"unicast\"\npeers = [\"192.0.2.11\"]\n[[instance.vip]]\naddress = \"192.0.2.20/24\"",
        ),
        "V-01",
    );
}

#[test]
fn v02_rejects_a_configured_priority_of_zero() {
    assert_rule(
        "schema_version = 1\n[node]\nname = \"n\"\n[[instance]]\nname = \"a\"\ninterface = \"eth0\"\nvrid = 1\npriority = 0\nadvertisement_interval = \"1s\"\n[instance.network]\nmode = \"unicast\"\npeers = [\"192.0.2.11\"]\n[[instance.vip]]\naddress = \"192.0.2.10/24\"",
        "V-02",
    );
}

#[test]
fn v03_rejects_mixed_families_in_one_instance() {
    let text = r#"
schema_version = 1
[node]
name = "node-a"
[[instance]]
name = "api"
interface = "eth0"
vrid = 1
advertisement_interval = "1s"
[instance.network]
mode = "unicast"
peers = ["192.0.2.11", "2001:db8::11"]
[[instance.vip]]
address = "192.0.2.10/24"
[[instance.vip]]
address = "2001:db8::10/64"
"#;
    assert_rule(text, "V-03");
}

#[test]
fn v04_rejects_an_advertisement_interval_outside_the_protocol_range() {
    for interval in ["9ms", "40951ms"] {
        let text = VALID.replace(
            "advertisement_interval = \"1s\"",
            &format!("advertisement_interval = \"{interval}\""),
        );
        assert_rule(&text, "V-04");
    }
}

#[test]
fn v05_rejects_duplicate_instance_names() {
    let text = r#"
schema_version = 1
[node]
name = "node-a"
[[instance]]
name = "api"
interface = "eth0"
vrid = 1
advertisement_interval = "1s"
[instance.network]
mode = "unicast"
peers = ["192.0.2.11"]
[[instance.vip]]
address = "192.0.2.10/24"
[[instance]]
name = "api"
interface = "eth1"
vrid = 2
advertisement_interval = "1s"
[instance.network]
mode = "unicast"
peers = ["192.0.2.11"]
[[instance.vip]]
address = "192.0.2.20/24"
"#;
    assert_rule(text, "V-05");
}

#[test]
fn v06_rejects_a_duplicate_interface_and_vrid_pair() {
    let text = r#"
schema_version = 1
[node]
name = "node-a"
[[instance]]
name = "one"
interface = "eth0"
vrid = 7
advertisement_interval = "1s"
[instance.network]
mode = "unicast"
peers = ["192.0.2.11"]
[[instance.vip]]
address = "192.0.2.10/24"
[[instance]]
name = "two"
interface = "eth0"
vrid = 7
advertisement_interval = "1s"
[instance.network]
mode = "unicast"
peers = ["192.0.2.11"]
[[instance.vip]]
address = "192.0.2.20/24"
"#;
    assert_rule(text, "V-06");
}

#[test]
fn v07_rejects_a_vip_family_without_a_matching_peer() {
    let text = VALID.replace("peers = [\"192.0.2.11\"]", "peers = [\"2001:db8::11\"]");
    let rules = rules_for(&text, &ValidationContext::permissive());
    assert!(rules.contains(&"V-07"), "expected V-07, got {rules:?}");
}

#[test]
fn v08_rejects_a_peer_configured_on_this_node() {
    let local: Vec<IpAddr> = vec!["192.0.2.11".parse().expect("valid address")];
    let context = ValidationContext {
        local_addresses: &local,
        ..ValidationContext::permissive()
    };
    let rules = rules_for(VALID, &context);
    assert!(rules.contains(&"V-08"), "expected V-08, got {rules:?}");
}

#[test]
fn v09_rejects_a_multicast_address_as_a_unicast_peer() {
    let text = VALID.replace("peers = [\"192.0.2.11\"]", "peers = [\"224.0.0.18\"]");
    assert_rule(&text, "V-09");
}

#[test]
fn v10_rejects_a_preemption_delay_without_preemption() {
    let text = VALID.replace(
        "advertisement_interval = \"1s\"",
        "advertisement_interval = \"1s\"\npreempt = false\npreempt_delay = \"30s\"",
    );
    assert_rule(&text, "V-10");
}

#[test]
fn v11_rejects_an_instance_without_vips() {
    let text = r#"
schema_version = 1
[node]
name = "node-a"
[[instance]]
name = "api"
interface = "eth0"
vrid = 1
advertisement_interval = "1s"
[instance.network]
mode = "unicast"
peers = ["192.0.2.11"]
"#;
    assert_rule(text, "V-11");
}

#[test]
fn v12_rejects_a_vip_shared_between_instances() {
    let text = r#"
schema_version = 1
[node]
name = "node-a"
[[instance]]
name = "one"
interface = "eth0"
vrid = 1
advertisement_interval = "1s"
[instance.network]
mode = "unicast"
peers = ["192.0.2.11"]
[[instance.vip]]
address = "192.0.2.10/24"
[[instance]]
name = "two"
interface = "eth1"
vrid = 2
advertisement_interval = "1s"
[instance.network]
mode = "unicast"
peers = ["192.0.2.11"]
[[instance.vip]]
address = "192.0.2.10/24"
"#;
    assert_rule(text, "V-12");
}

#[test]
fn v12_rejects_a_vip_configured_twice_in_one_instance() {
    let text = VALID.replace(
        "[[instance.vip]]\naddress = \"192.0.2.10/24\"",
        "[[instance.vip]]\naddress = \"192.0.2.10/24\"\n[[instance.vip]]\naddress = \"192.0.2.10/24\"",
    );
    assert_rule(&text, "V-12");
}

#[test]
fn v13_rejects_a_malformed_or_default_route_vip() {
    for address in ["192.0.2.10", "192.0.2.10/33", "192.0.2.10/0"] {
        let text = VALID.replace("192.0.2.10/24", address);
        assert_rule(&text, "V-13");
    }
}

#[test]
fn v14_rejects_a_non_positive_timeout_interval_or_ordering() {
    let check = |timeout: &str, interval: &str| {
        format!(
            "{VALID}\n[[instance.check]]\nname = \"api\"\ntype = \"tcp\"\naddress = \"127.0.0.1:5432\"\ntimeout = \"{timeout}\"\ninterval = \"{interval}\"\nfailure_threshold = 1\nsuccess_threshold = 1"
        )
    };
    assert_rule(&check("0ms", "1s"), "V-14");
    assert_rule(&check("500ms", "0s"), "V-14");
    assert_rule(&check("2s", "1s"), "V-14");
}

#[test]
fn v15_rejects_a_zero_threshold() {
    let text = format!(
        "{VALID}\n[[instance.check]]\nname = \"api\"\ntype = \"tcp\"\naddress = \"127.0.0.1:5432\"\ntimeout = \"500ms\"\ninterval = \"1s\"\nfailure_threshold = 0\nsuccess_threshold = 1"
    );
    assert_rule(&text, "V-15");
}

#[test]
fn v16_rejects_a_minimum_priority_under_fail_closed() {
    let text = format!(
        "{VALID}\n[instance.health]\nfailure_policy = \"fail_closed\"\nminimum_effective_priority = 100"
    );
    assert_rule(&text, "V-16");
}

#[test]
fn v17_rejects_options_that_mean_nothing_under_manual() {
    let text = format!(
        "{VALID}\n[instance.health]\nfailure_policy = \"manual\"\nall_checks_required = true"
    );
    assert_rule(&text, "V-17");
}

#[test]
fn v18_rejects_fail_closed_only_options_under_weighted() {
    let text = format!("{VALID}\n[instance.health]\nall_checks_required = true");
    assert_rule(&text, "V-18");
}

#[test]
fn v19_rejects_electoral_checks_under_manual() {
    let text = format!(
        "{VALID}\n[instance.health]\nfailure_policy = \"manual\"\n[[instance.check]]\nname = \"api\"\ntype = \"tcp\"\naddress = \"127.0.0.1:5432\"\ntimeout = \"500ms\"\ninterval = \"1s\"\nfailure_threshold = 1\nsuccess_threshold = 1\nweight = 50"
    );
    assert_rule(&text, "V-19");
}

#[test]
fn v20_rejects_a_total_weight_above_255() {
    let text = format!(
        "{VALID}\n[[instance.check]]\nname = \"a\"\ntype = \"tcp\"\naddress = \"127.0.0.1:1\"\ntimeout = \"500ms\"\ninterval = \"1s\"\nfailure_threshold = 1\nsuccess_threshold = 1\nweight = 200\n[[instance.check]]\nname = \"b\"\ntype = \"tcp\"\naddress = \"127.0.0.1:2\"\ntimeout = \"500ms\"\ninterval = \"1s\"\nfailure_threshold = 1\nsuccess_threshold = 1\nweight = 200"
    );
    assert_rule(&text, "V-20");
}

#[test]
fn v21_rejects_a_command_check_without_the_feature_and_allow_list() {
    let text = format!(
        "{VALID}\n[[instance.check]]\nname = \"script\"\ntype = \"command\"\ncommand = [\"/bin/true\"]\ntimeout = \"500ms\"\ninterval = \"1s\"\nfailure_threshold = 1\nsuccess_threshold = 1"
    );
    assert_rule(&text, "V-21");

    let enabled = ValidationContext {
        command_checks_enabled: true,
        ..ValidationContext::permissive()
    };
    assert!(
        rules_for(&text, &enabled).contains(&"V-21"),
        "allow-list is still required"
    );

    let text_with_relative_path = text.replace("/bin/true", "bin/true").replace(
        "command = [\"bin/true\"]",
        "command = [\"bin/true\"]\nallow_paths = [\"bin/true\"]",
    );
    assert!(rules_for(&text_with_relative_path, &enabled).contains(&"V-21"));
}

#[test]
fn v22_rejects_a_missing_interface_unless_binding_is_deferred() {
    let names: BTreeSet<String> = ["eth9".to_owned()].into_iter().collect();
    let context = ValidationContext {
        interfaces: &KnownInterfaces { names: &names },
        ..ValidationContext::permissive()
    };
    assert!(rules_for(VALID, &context).contains(&"V-22"));

    let deferred = VALID.replace(
        "advertisement_interval = \"1s\"",
        "advertisement_interval = \"1s\"\ndefer_interface_binding = true",
    );
    assert!(!rules_for(&deferred, &context).contains(&"V-22"));
}

#[test]
fn v23_rejects_a_check_missing_the_keys_its_type_requires() {
    let text = format!(
        "{VALID}\n[[instance.check]]\nname = \"api\"\ntype = \"http\"\ntimeout = \"500ms\"\ninterval = \"1s\"\nfailure_threshold = 1\nsuccess_threshold = 1"
    );
    assert_rule(&text, "V-23");

    let unknown = format!(
        "{VALID}\n[[instance.check]]\nname = \"api\"\ntype = \"telepathy\"\ntimeout = \"500ms\"\ninterval = \"1s\"\nfailure_threshold = 1\nsuccess_threshold = 1"
    );
    assert_rule(&unknown, "V-23");
}

#[test]
fn v24_rejects_a_multicast_ttl_other_than_255() {
    let text = VALID.replace("mode = \"unicast\"", "mode = \"multicast\"")
        + "\n[instance.network.multicast]\nttl = 1\n";
    assert_rule(&text, "V-24");

    let valid = VALID.replace("mode = \"unicast\"", "mode = \"multicast\"");
    assert_no_rules(&valid);
}

#[test]
fn v25_rejects_more_than_sixty_four_checks_in_one_instance() {
    let mut text = VALID.to_owned();
    for index in 0..65 {
        let port = index + 1;
        write!(
            text,
            "\n[[instance.check]]\nname = \"c{index}\"\ntype = \"tcp\"\naddress = \"127.0.0.1:{port}\"\ntimeout = \"500ms\"\ninterval = \"1s\"\nfailure_threshold = 1\nsuccess_threshold = 1"
        )
        .expect("writing to a String cannot fail");
    }
    assert_rule(&text, "V-25");
}

#[test]
fn v27_rejects_more_than_256_instances() {
    let mut text = String::from("schema_version = 1\n[node]\nname = \"node-a\"\n");
    for index in 0..257 {
        let vrid = (index % 254) + 1;
        let last_octet = (index % 200) + 20;
        write!(
            text,
            "\n[[instance]]\nname = \"i{index}\"\ninterface = \"eth0\"\nvrid = {vrid}\nadvertisement_interval = \"1s\"\n[instance.network]\nmode = \"unicast\"\npeers = [\"192.0.2.11\"]\n[[instance.vip]]\naddress = \"192.0.2.{last_octet}/24\"\n"
        )
        .expect("writing to a String cannot fail");
    }
    assert_rule(&text, "V-27");
}

#[test]
fn v28_rejects_an_unsupported_schema_version() {
    let text = VALID.replace("schema_version = 1", "schema_version = 99");
    assert_rule(&text, "V-28");
}

#[test]
fn v29_rejects_duplicate_check_names_in_one_instance() {
    let check = "\n[[instance.check]]\nname = \"same\"\ntype = \"tcp\"\naddress = \"127.0.0.1:1\"\ntimeout = \"500ms\"\ninterval = \"1s\"\nfailure_threshold = 1\nsuccess_threshold = 1";
    let text = format!("{VALID}{check}{check}");
    assert_rule(&text, "V-29");
}

#[test]
fn v30_rejects_inconsistent_metrics_settings() {
    let enabled = format!("{VALID}\n[metrics]\nenabled = true");
    assert_rule(&enabled, "V-30");

    let disabled = format!("{VALID}\n[metrics]\nenabled = false\nlisten = \"127.0.0.1:9900\"");
    assert_rule(&disabled, "V-30");
}

#[test]
fn v31_requires_at_least_one_instance() {
    assert_rule("schema_version = 1\n[node]\nname = \"node-a\"", "V-31");
}

#[test]
fn v32_rejects_an_empty_node_name() {
    let text = VALID.replace("name = \"node-a\"", "name = \"  \"");
    assert_rule(&text, "V-32");
}

#[test]
fn every_implemented_rule_has_a_test() {
    /// The rules this file exercises, kept explicit so that a newly implemented
    /// rule cannot be added without a test.
    const COVERED: &[&str] = &[
        "V-01", "V-02", "V-03", "V-04", "V-05", "V-06", "V-07", "V-08", "V-09", "V-10", "V-11",
        "V-12", "V-13", "V-14", "V-15", "V-16", "V-17", "V-18", "V-19", "V-20", "V-21", "V-22",
        "V-23", "V-24", "V-25", "V-27", "V-28", "V-29", "V-30", "V-31", "V-32",
    ];

    let config: Config = parse(VALID).expect("the reference document parses");
    assert_eq!(config.instances.len(), 1);
    assert_eq!(
        COVERED.len(),
        31,
        "V-26 is covered by the loader tests in src/loader.rs"
    );

    // Every rule the implementation can emit must be listed above.
    let mut emitted: BTreeSet<&'static str> = BTreeSet::new();
    let broken = VALID
        .replace("vrid = 42", "vrid = 0")
        .replace("peers = [\"192.0.2.11\"]", "peers = [\"224.0.0.18\"]");
    for violation in validate(
        &parse(&broken).expect("parses"),
        &ValidationContext::permissive(),
    )
    .expect_err("the broken document is invalid")
    {
        emitted.insert(violation.rule);
    }
    for rule in emitted {
        assert!(
            COVERED.contains(&rule),
            "rule {rule} has no test in this file"
        );
    }
}

#[test]
fn a_valid_document_with_observational_checks_is_accepted() {
    let text = format!(
        "{VALID}\n[[instance.check]]\nname = \"api\"\ntype = \"http\"\nurl = \"http://127.0.0.1:8080/ready\"\nexpected_status = [200]\ntimeout = \"500ms\"\ninterval = \"1s\"\nfailure_threshold = 3\nsuccess_threshold = 2"
    );
    let config = parse(&text).expect("parses");
    assert_eq!(config.instances[0].electoral_weight(), 0);
    assert_no_rules(&text);
}

#[test]
fn an_instance_probe_can_be_supplied_by_the_caller() {
    #[derive(Debug)]
    struct OnlyLoopback;
    impl InterfaceProbe for OnlyLoopback {
        fn interface_exists(&self, name: &str) -> bool {
            name == "lo"
        }
    }

    let context = ValidationContext {
        interfaces: &OnlyLoopback,
        ..ValidationContext::permissive()
    };
    assert!(rules_for(VALID, &context).contains(&"V-22"));
    let text = VALID.replace("interface = \"eth0\"", "interface = \"lo\"");
    assert!(!rules_for(&text, &context).contains(&"V-22"));
}

/// The reference document, kept as a fixture so that operators and tests read
/// the same file.
const FIXTURE: &str = include_str!("fixtures/basic.toml");

#[test]
fn the_shipped_fixture_is_valid() {
    let config =
        load_and_validate(FIXTURE, &ValidationContext::permissive()).expect("the fixture is valid");
    assert_eq!(config.node.name, "node-a");
    assert_eq!(config.instances.len(), 1);
    assert_eq!(config.instances[0].vrid, 42);
    assert_eq!(config.instances[0].vip_addresses(), ["192.0.2.10/24"]);
}

#[test]
fn the_fixture_agrees_with_the_inline_reference_document() {
    // The two must not drift: the inline document documents the rules, the
    // fixture is what a user copies.
    let from_fixture = parse(FIXTURE).expect("the fixture parses");
    let inline = parse(VALID).expect("the reference document parses");
    assert_eq!(from_fixture.instances.len(), inline.instances.len());
    assert_eq!(from_fixture.instances[0].vrid, inline.instances[0].vrid);
    assert_eq!(
        from_fixture.instances[0].priority,
        inline.instances[0].priority
    );
}
