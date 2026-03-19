fn main() {
    glib_build_tools::compile_resources(
        &["data"],
        "data/io.github.up.gresource.xml",
        "compiled.gresource",
    );
}
