// No `use` statement for `foo::bar` on purpose (G1 recipe fixture): the call
// is only reachable via the fully path-qualified form below.
pub fn use_bar() {
    crate::foo::bar();
}
