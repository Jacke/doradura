// Cache invalidation: 2026-02-05-v3
fn main() {
    cc::Build::new()
        .file("c_code/foo.c")
        .file("c_code/bar.c")
        .compile("foo_bar");
}
