const PROJECT_MIT: &str = include_str!("../LICENSE-MIT");
const PROJECT_APACHE: &str = include_str!("../LICENSE-APACHE");
const INTER_OFL: &str = include_str!("../assets/Inter-OFL.txt");
const JBM_OFL: &str = include_str!("../assets/JetBrainsMono-OFL.txt");
const THIRD_PARTY: &str = include_str!("../THIRD-PARTY-LICENSES.md");

pub fn print() {
    println!("# mdv");
    println!();
    println!("Dual-licensed under MIT or Apache-2.0. You may choose either.");
    println!();
    println!("## MIT");
    println!();
    println!("{}", PROJECT_MIT);
    println!("## Apache-2.0");
    println!();
    println!("{}", PROJECT_APACHE);
    println!();
    println!("# Bundled fonts");
    println!();
    println!("## Inter (https://github.com/rsms/inter)");
    println!();
    println!("{}", INTER_OFL);
    println!();
    println!("## JetBrains Mono (https://github.com/JetBrains/JetBrainsMono)");
    println!();
    println!("{}", JBM_OFL);
    println!();
    println!("{}", THIRD_PARTY);
}
