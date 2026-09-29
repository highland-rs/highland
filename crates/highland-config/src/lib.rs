// Rust guideline compliant 2026-09-27

//! Configuration model, parser, and validation for Highland.
//!
//! The configuration is a typed model, not a bag of strings
//! (SPEC.md, `D-05`). Parsing, schema validation, and semantic validation are
//! separate steps, and every semantic failure names the `V-nn` rule it
//! violated, so that a diagnostic can be traced to a specification
//! requirement and to a test fixture (SPEC.md, §10.4).
//!
//! # Example
//!
//! ```
//! use highland_config::{ValidationContext, load_and_validate};
//!
//! let text = r#"
//! schema_version = 1
//!
//! [node]
//! name = "node-a"
//!
//! [[instance]]
//! name = "api"
//! interface = "eth0"
//! vrid = 42
//! priority = 150
//!
//! [instance.network]
//! mode = "unicast"
//! peers = ["192.0.2.11"]
//!
//! [[instance.vip]]
//! address = "192.0.2.10/24"
//! "#;
//!
//! let config = load_and_validate(text, &ValidationContext::permissive())
//!     .expect("the fixture is valid");
//! assert_eq!(config.node.name, "node-a");
//! assert_eq!(config.instances[0].vrid, 42);
//! ```

#![deny(missing_docs)]
#![forbid(unsafe_code)]

mod duration;
mod error;
mod loader;
mod model;
mod validation;

pub use duration::{DurationSpec, parse_duration};
pub use error::{
    ConfigError, ConfigViolation, ConfigViolations, DurationParseError, SourceLocation, Violation,
};
pub use loader::{MAX_CONFIG_BYTES, load, load_and_validate, parse};
pub use model::{
    CHECK_KEY_TYPES, CheckConfig, Config, ControlConfig, FailurePolicy, Family, HealthConfig,
    InstanceConfig, InstanceLimits, LoggingConfig, MetricsConfig, MulticastConfig, NetworkConfig,
    NetworkMode, NodeConfig, SUPPORTED_SCHEMA_VERSION, SchemaVersion, UNIMPLEMENTED_CHECK_TYPES,
    VipConfig, unimplemented_check_reason,
};
pub use validation::{
    AnyInterface, InterfaceProbe, KnownInterfaces, MAX_ADVERTISEMENT_INTERVAL, MAX_INSTANCES,
    MAX_TOTAL_WEIGHT, ValidationContext, validate,
};
