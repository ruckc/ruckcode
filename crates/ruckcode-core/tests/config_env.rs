//! Pins the environment-driven parts of config loading. One test function, because it mutates
//! the process environment.

use ruckcode_core::config::Config;

#[test]
fn profile_env_is_the_last_layer_and_config_dir_env_locates_the_user_config() {
    let user = tempfile::tempdir().unwrap();
    let proj = tempfile::tempdir().unwrap();
    std::fs::write(
        user.path().join("config.toml"),
        "default_profile = \"u\"\n[profiles.u]\nprovider = \"mock\"\nmodel = \"m\"\n[profiles.v]\nprovider = \"mock\"\nmodel = \"m2\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(proj.path().join(".ruckcode")).unwrap();
    std::fs::write(
        proj.path().join(".ruckcode/config.toml"),
        "default_profile = \"openai\"\n",
    )
    .unwrap();

    std::env::remove_var("RUCKCODE_PROFILE");
    std::env::set_var("RUCKCODE_CONFIG_DIR", user.path());
    // No explicit user dir: the env var finds it. The project layer beats the user's.
    let c = Config::load(proj.path(), None).unwrap();
    assert_eq!(c.default_profile, "openai");
    assert!(c.profiles.contains_key("u"));

    // RUCKCODE_PROFILE beats the project config.
    std::env::set_var("RUCKCODE_PROFILE", "v");
    let c = Config::load(proj.path(), None).unwrap();
    assert_eq!(c.default_profile, "v");
    assert_eq!(c.profiles["v"].model, "m2");

    // An empty value is ignored.
    std::env::set_var("RUCKCODE_PROFILE", "");
    let c = Config::load(proj.path(), None).unwrap();
    assert_eq!(c.default_profile, "openai");

    // An unknown profile name is a validation error.
    std::env::set_var("RUCKCODE_PROFILE", "nope");
    let e = Config::load(proj.path(), None).unwrap_err();
    assert_eq!(e.to_string(), "default_profile `nope` is not defined");

    // An explicit user dir wins over the env var.
    let other = tempfile::tempdir().unwrap();
    std::env::remove_var("RUCKCODE_PROFILE");
    let c = Config::load(proj.path(), Some(other.path())).unwrap();
    assert!(!c.profiles.contains_key("u"));

    std::env::remove_var("RUCKCODE_CONFIG_DIR");
}
