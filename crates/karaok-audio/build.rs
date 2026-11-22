fn main() {
    #[cfg(target_os = "macos")]
    {
        println!("cargo:rerun-if-changed=native/av_mic.m");
        cc::Build::new()
            .file("native/av_mic.m")
            .flag("-fobjc-arc")
            .flag("-mmacosx-version-min=11.0")
            .flag("-Wno-deprecated-declarations")
            .compile("karaok_av_mic");

        println!("cargo:rustc-link-lib=framework=AVFoundation");
        println!("cargo:rustc-link-lib=framework=CoreMedia");
        println!("cargo:rustc-link-lib=framework=CoreAudio");
        println!("cargo:rustc-link-lib=framework=Foundation");
        println!("cargo:rustc-link-lib=framework=IOKit");
    }
}
