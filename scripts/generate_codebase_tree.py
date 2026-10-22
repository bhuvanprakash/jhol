import os

# Directories that contain temporary, benchmark artifact, or cache files
ignore_dirs = {
    '.git', 'target', 'node_modules', '.cargo', 'jhol_cache_1', 'jhol_cache_2', 
    'store_unpacked', 'store_meta', 'store_tar', '.gemini', '__pycache__', 
    '.system_generated', 'dist', 'build', 'brutal_audit', 'results', 
    'results_prw_check', 'prw-baseline', 'brutal_warm_audit'
}

def walk_tree(dir_path, prefix=''):
    try:
        entries = sorted(os.listdir(dir_path))
    except Exception:
        return []
    entries = [e for e in entries if e not in ignore_dirs and not e.startswith('.') and not e.endswith('.pyc')]
    
    # Filter out binary-packages or fixture huge subdirs if any
    if 'binary-packages' in dir_path and len(entries) > 10:
        sample = entries[:3]
        res = []
        for s in sample:
            res.append(f"{prefix}├── {s}")
        res.append(f"{prefix}└── ... [{len(entries) - 3} more pre-packed binary tarballs/entries]")
        return res

    lines = []
    for i, entry in enumerate(entries):
        full_path = os.path.join(dir_path, entry)
        is_last = (i == len(entries) - 1)
        connector = '└── ' if is_last else '├── '
        if os.path.isdir(full_path):
            lines.append(f"{prefix}{connector}{entry}/")
            ext = '    ' if is_last else '│   '
            lines.extend(walk_tree(full_path, prefix + ext))
        else:
            lines.append(f"{prefix}{connector}{entry}")
    return lines

def generate_markdown():
    out = []
    out.append("# Jhol Codebase File Tree & Architectural Map\n")
    out.append("This document contains the complete, curated structural tree of every folder and source file in the **Jhol** repository.\n")
    
    out.append("## 1. Complete Repository Directory Tree\n")
    out.append("```text")
    out.append("jhol/")
    out.extend(walk_tree('.'))
    out.append("```\n")

    out.append("## 2. Categorized Functional Subsystem Map & File Breakdown\n")
    
    out.append("### A. CLI Binary Layer (`src/`)\n")
    out.append("- `src/main.rs` — Main CLI entrypoint, argument parsing (Clap), subcommand dispatch, Unix daemon service.\n")
    out.append("- `src/tui.rs` — Interactive Terminal UI (`jhol gui`) built with Ratatui.\n")
    out.append("- `src/bin/jhol-fast.rs` — Alternative fast-path CLI binary.\n")
    out.append("- `src/jhol.sh` — Helper wrapper shell script.\n")

    out.append("\n### B. Core Library Modules (`crates/jhol-core/src/`)\n")
    
    core_files = sorted([f for f in os.listdir('crates/jhol-core/src') if not f.startswith('.') and os.path.isfile(os.path.join('crates/jhol-core/src', f))])
    core_dirs = sorted([d for d in os.listdir('crates/jhol-core/src') if not d.startswith('.') and os.path.isdir(os.path.join('crates/jhol-core/src', d))])

    out.append("#### Top-Level Core Modules:\n")
    for f in core_files:
        out.append(f"- `crates/jhol-core/src/{f}`")
    
    out.append("\n#### Core Subdirectories:\n")
    for d in core_dirs:
        out.append(f"##### Subsystem `crates/jhol-core/src/{d}/`:")
        sub_files = sorted([f for f in os.listdir(os.path.join('crates/jhol-core/src', d)) if not f.startswith('.')])
        for sf in sub_files:
            subpath = os.path.join('crates/jhol-core/src', d, sf)
            if os.path.isdir(subpath):
                out.append(f"- `{subpath}/`")
                for ssf in sorted(os.listdir(subpath)):
                    if not ssf.startswith('.'):
                        out.append(f"  - `{subpath}/{ssf}`")
            else:
                out.append(f"- `crates/jhol-core/src/{d}/{sf}`")
        out.append("")

    out.append("\n### C. Integration Test Suites & Fixtures (`tests/`)\n")
    test_entries = sorted([f for f in os.listdir('tests') if not f.startswith('.')])
    for t in test_entries:
        t_path = os.path.join('tests', t)
        if os.path.isdir(t_path):
            out.append(f"- `{t_path}/`")
            for sub in sorted(os.listdir(t_path)):
                if not sub.startswith('.'):
                    out.append(f"  - `{t_path}/{sub}`")
        else:
            out.append(f"- `tests/{t}`")

    out.append("\n### D. Documentation (`docs/`, `Documentation/`, Root Docs)\n")
    for root_doc in sorted([f for f in os.listdir('.') if f.endswith('.md') or f.endswith('.pdf')]):
        out.append(f"- `{root_doc}`")
    if os.path.exists('docs'):
        for root, dirs, files in os.walk('docs'):
            dirs.sort()
            for f in sorted(files):
                if not f.startswith('.'):
                    rel = os.path.join(root, f)
                    out.append(f"- `{rel}`")
    if os.path.exists('Documentation'):
        for root, dirs, files in os.walk('Documentation'):
            dirs.sort()
            for f in sorted(files):
                if not f.startswith('.'):
                    rel = os.path.join(root, f)
                    out.append(f"- `{rel}`")

    out.append("\n### E. Benchmarks & Scripts (`benchmarks/`, `scripts/`, `runtime-benchmarks/`)\n")
    for s in sorted(os.listdir('scripts')):
        if not s.startswith('.') and not s.startswith('__'):
            s_path = os.path.join('scripts', s)
            if os.path.isdir(s_path):
                out.append(f"- `{s_path}/`")
                for sf in sorted(os.listdir(s_path)):
                    if not sf.startswith('.'):
                        out.append(f"  - `{s_path}/{sf}`")
            else:
                out.append(f"- `scripts/{s}`")

    with open('CODEBASE_STRUCTURE_TREE.md', 'w') as f:
        f.write('\n'.join(out))
    print("Successfully generated clean CODEBASE_STRUCTURE_TREE.md")

if __name__ == '__main__':
    generate_markdown()
