fn main() {
    let mut cmd = std::process::Command::new("sh");
    cmd.arg("-c").arg("echo synthetic build hook");
}

