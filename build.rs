fn main() {
    println!("cargo:rerun-if-changed=varita.ico");
    let mut r = winresource::WindowsResource::new();
    r.set_icon("varita.ico");
    r.set("FileDescription", "BW2Color Movie – black-and-white film colorizer");
    r.set("ProductName", "BW2Color Movie");
    r.compile().expect("kuvakkeen liittäminen epäonnistui");
}
