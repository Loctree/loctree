pub struct Resolver;

impl Resolver {
    pub fn normalize<'a>(&self, raw: &'a str) -> String {
        let flipped = raw.replace('\\', "/");
        flipped.trim_start_matches("./").to_string()
    }

    pub fn sibling_marker(&self) -> &'static str {
        "sibling"
    }
}
