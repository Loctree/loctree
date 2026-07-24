pub fn format_build_version(package_version: &str, commit: &str, dirty: bool) -> String {
    if commit == "unknown" {
        package_version.to_owned()
    } else if dirty {
        format!("{package_version}+g{commit}.dirty")
    } else {
        format!("{package_version}+g{commit}")
    }
}
