#[cfg(test)]
mod tests {
    use loctree_mcp::extract_symbol;

    #[test]
    fn test_tokenizer() {
        assert_eq!(
            extract_symbol("fn heartbeat_enabled(&self) -> Option<bool> {"),
            "heartbeat_enabled"
        );
        assert_eq!(
            extract_symbol("pub fn extract_something() {"),
            "extract_something"
        );
        assert_eq!(
            extract_symbol("pub(crate) fn internal_thing<T>() {"),
            "internal_thing"
        );
        assert_eq!(
            extract_symbol("async fn do_async_work() {"),
            "do_async_work"
        );
        assert_eq!(
            extract_symbol("fn with_where_clause() where T: Clone {"),
            "with_where_clause"
        );
        assert_eq!(extract_symbol("struct MyStruct {"), "MyStruct");
        assert_eq!(extract_symbol("enum MyEnum {"), "MyEnum");
    }
}
