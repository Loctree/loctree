#[path = "../build_support.rs"]
mod build_support;

#[test]
fn formatter_distinguishes_checkout_states() {
    assert_eq!(
        build_support::format_build_version("0.13.1", "deadbeef", false),
        "0.13.1+gdeadbeef"
    );
    assert_eq!(
        build_support::format_build_version("0.13.1", "deadbeef", true),
        "0.13.1+gdeadbeef.dirty"
    );
    assert_eq!(
        build_support::format_build_version("0.13.1", "unknown", true),
        "0.13.1"
    );
}
