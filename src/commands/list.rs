use mumei_core::registry;
use std::path::Path;

pub(crate) fn cmd_list() {
    let packages = registry::list_packages();

    if packages.is_empty() {
        println!("📦 No packages in local registry (~/.mumei/registry.json).");
        println!();
        println!("   To publish a package:");
        println!("     cd <your-project>");
        println!("     mumei publish");
        return;
    }

    println!("📦 Local Registry — {} package(s):", packages.len());
    println!();

    for (name, entry) in &packages {
        let verified_icon = entry
            .versions
            .get(&entry.latest)
            .map(|v| if v.verified { " ✅" } else { "" })
            .unwrap_or("");
        println!("  {} v{}{}", name, entry.latest, verified_icon);

        // Show all versions with details
        let mut versions: Vec<(&String, &registry::VersionEntry)> = entry.versions.iter().collect();
        versions.sort_by(|a, b| a.0.cmp(b.0));

        for (ver, ver_entry) in &versions {
            let current = if *ver == &entry.latest {
                " (latest)"
            } else {
                ""
            };
            let verified = if ver_entry.verified {
                "verified"
            } else {
                "unverified"
            };
            let path_exists = if Path::new(&ver_entry.path).exists() {
                ""
            } else {
                " [missing]"
            };
            println!(
                "    v{}: {} atoms, {}, published {}{}{}",
                ver, ver_entry.atom_count, verified, ver_entry.published_at, current, path_exists
            );
        }
        println!();
    }

    println!("   Registry path: {}", registry::registry_path().display());
}

// =============================================================================
// mumei repl — Interactive REPL (Read-Eval-Print Loop)
// =============================================================================
