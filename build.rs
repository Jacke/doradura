fn main() {
    cc::Build::new()
        .file("c_code/foo.c")
        .file("c_code/bar.c")
        .compile("foo_bar");
}
