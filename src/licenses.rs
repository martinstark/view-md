const INTER_OFL: &str = include_str!("../assets/Inter-OFL.txt");
const JBM_OFL: &str = include_str!("../assets/JetBrainsMono-OFL.txt");

pub fn print() {
    println!("mdv bundles the following fonts. Their licenses are reproduced below.\n");
    println!("================================================================");
    println!("Inter (https://github.com/rsms/inter)");
    println!("================================================================\n");
    println!("{}", INTER_OFL);
    println!("\n================================================================");
    println!("JetBrains Mono (https://github.com/JetBrains/JetBrainsMono)");
    println!("================================================================\n");
    println!("{}", JBM_OFL);
}
