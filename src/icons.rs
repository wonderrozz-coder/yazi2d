//! File and folder icons, taken from Yazi's default theme.
//!
//! Yazi draws a Nerd Font glyph in front of every entry, coloured per type: a
//! folder glyph for directories, a name-specific one for `Documents` or
//! `.git`, and an extension-specific one for `*.rs` or `*.png`. The tables
//! below are generated from `yazi-config/preset/theme-dark.toml`
//! (`[icon] dirs/files/exts`), so with a Nerd Font installed the listing looks
//! exactly like Yazi's.
//!
//! Terminals without a patched font render those code points as boxes. For
//! them the settings file has `icons = false`, which switches the listing to
//! plain ASCII markers. The block-art icons in [`art`] are still used by the
//! thumbnail grid, where a single glyph is too small to recognise.
//!
//! Lookup order matches Yazi's: exact file name, then extension, then the
//! generic folder/file glyph.

use std::path::Path;

use crate::colors::FileGroup;

/// Glyph for a folder whose name is not in [`DIRS`].
pub const FOLDER: char = '\u{f115}';
/// Glyph for a file whose name and extension are not in the tables.
pub const FILE: char = '\u{f016}';
/// Glyph for the synthetic `..` entry.
pub const PARENT: char = '\u{f062}';
/// Colour used when a table has no entry for the name.
pub const DEFAULT_COLOR: &str = "#7f8c8d";

/// Well-known folder names; matched case-insensitively.
static DIRS: [(&str, char, &str); 14] = [
    (".config", '', "#ff9800"),
    (".git", '', "#00bcd4"),
    (".github", '', "#03a9f4"),
    (".npm", '', "#03a9f4"),
    ("Desktop", '', "#00bcd4"),
    ("Development", '', "#00bcd4"),
    ("Documents", '', "#00bcd4"),
    ("Downloads", '', "#00bcd4"),
    ("Library", '', "#00bcd4"),
    ("Movies", '', "#00bcd4"),
    ("Music", '', "#00bcd4"),
    ("Pictures", '', "#00bcd4"),
    ("Public", '', "#00bcd4"),
    ("Videos", '', "#00bcd4"),
];

/// Exact file names that deserve a specific glyph.
static FILES: [(&str, char, &str); 214] = [
    (".babelrc", '', "#cbcb41"),
    (".bash_profile", '', "#89e051"),
    (".bashrc", '', "#89e051"),
    (".clang-format", '', "#6d8086"),
    (".clang-tidy", '', "#6d8086"),
    (".codespellrc", '󰓆', "#35da60"),
    (".condarc", '', "#43b02a"),
    (".dockerignore", '󰡨', "#458ee6"),
    (".ds_store", '', "#41535b"),
    (".editorconfig", '', "#fff2f2"),
    (".env", '', "#faf743"),
    (".eslintignore", '', "#4b32c3"),
    (".eslintrc", '', "#4b32c3"),
    (".git-blame-ignore-revs", '', "#f54d27"),
    (".gitattributes", '', "#f54d27"),
    (".gitconfig", '', "#f54d27"),
    (".gitignore", '', "#f54d27"),
    (".gitlab-ci.yml", '', "#e24329"),
    (".gitmodules", '', "#f54d27"),
    (".gtkrc-2.0", '', "#ffffff"),
    (".gvimrc", '', "#019833"),
    (".justfile", '', "#6d8086"),
    (".luacheckrc", '', "#00a2ff"),
    (".luaurc", '', "#00a2ff"),
    (".mailmap", '󰊢', "#f54d27"),
    (".nanorc", '', "#440077"),
    (".npmignore", '', "#e8274b"),
    (".npmrc", '', "#e8274b"),
    (".nuxtrc", '󱄆', "#00c58e"),
    (".nvmrc", '', "#5fa04e"),
    (".pnpmfile.cjs", '', "#f9ad02"),
    (".pre-commit-config.yaml", '󰛢', "#f8b424"),
    (".prettierignore", '', "#4285f4"),
    (".prettierrc", '', "#4285f4"),
    (".prettierrc.cjs", '', "#4285f4"),
    (".prettierrc.js", '', "#4285f4"),
    (".prettierrc.json", '', "#4285f4"),
    (".prettierrc.json5", '', "#4285f4"),
    (".prettierrc.mjs", '', "#4285f4"),
    (".prettierrc.toml", '', "#4285f4"),
    (".prettierrc.yaml", '', "#4285f4"),
    (".prettierrc.yml", '', "#4285f4"),
    (".pylintrc", '', "#6d8086"),
    (".settings.json", '', "#854cc7"),
    (".SRCINFO", '󰣇', "#0f94d2"),
    (".vimrc", '', "#019833"),
    (".Xauthority", '', "#e54d18"),
    (".xinitrc", '', "#e54d18"),
    (".Xresources", '', "#e54d18"),
    (".xsession", '', "#e54d18"),
    (".zprofile", '', "#89e051"),
    (".zshenv", '', "#89e051"),
    (".zshrc", '', "#89e051"),
    ("_gvimrc", '', "#019833"),
    ("_vimrc", '', "#019833"),
    ("AUTHORS", '', "#a172ff"),
    ("AUTHORS.txt", '', "#a172ff"),
    ("brewfile", '', "#701516"),
    ("bspwmrc", '', "#2f2f2f"),
    ("build", '', "#89e051"),
    ("build.gradle", '', "#005f87"),
    ("build.zig.zon", '', "#f69a1b"),
    ("bun.lock", '', "#eadcd1"),
    ("bun.lockb", '', "#eadcd1"),
    ("cantorrc", '', "#1c99f3"),
    ("checkhealth", '󰓙', "#75b4fb"),
    ("cmakelists.txt", '', "#dce3eb"),
    ("code_of_conduct", '', "#e41662"),
    ("code_of_conduct.md", '', "#e41662"),
    ("commit_editmsg", '', "#f54d27"),
    ("commitlint.config.js", '󰜘', "#2b9689"),
    ("commitlint.config.ts", '󰜘', "#2b9689"),
    ("compose.yaml", '󰡨', "#458ee6"),
    ("compose.yml", '󰡨', "#458ee6"),
    ("config", '', "#6d8086"),
    ("containerfile", '󰡨', "#458ee6"),
    ("copying", '', "#cbcb41"),
    ("copying.lesser", '', "#cbcb41"),
    ("Directory.Build.props", '', "#00a2ff"),
    ("Directory.Build.targets", '', "#00a2ff"),
    ("Directory.Packages.props", '', "#00a2ff"),
    ("docker-compose.yaml", '󰡨', "#458ee6"),
    ("docker-compose.yml", '󰡨', "#458ee6"),
    ("dockerfile", '󰡨', "#458ee6"),
    ("eslint.config.cjs", '', "#4b32c3"),
    ("eslint.config.js", '', "#4b32c3"),
    ("eslint.config.mjs", '', "#4b32c3"),
    ("eslint.config.ts", '', "#4b32c3"),
    ("ext_typoscript_setup.txt", '', "#ff8700"),
    ("favicon.ico", '', "#cbcb41"),
    ("fp-info-cache", '', "#ffffff"),
    ("fp-lib-table", '', "#ffffff"),
    ("FreeCAD.conf", '', "#cb333b"),
    ("Gemfile", '', "#701516"),
    ("gnumakefile", '', "#6d8086"),
    ("go.mod", '', "#00add8"),
    ("go.sum", '', "#00add8"),
    ("go.work", '', "#00add8"),
    ("gradle-wrapper.properties", '', "#005f87"),
    ("gradle.properties", '', "#005f87"),
    ("gradlew", '', "#005f87"),
    ("groovy", '', "#4a687c"),
    ("gruntfile.babel.js", '', "#e37933"),
    ("gruntfile.coffee", '', "#e37933"),
    ("gruntfile.js", '', "#e37933"),
    ("gruntfile.ts", '', "#e37933"),
    ("gtkrc", '', "#ffffff"),
    ("gulpfile.babel.js", '', "#cc3e44"),
    ("gulpfile.coffee", '', "#cc3e44"),
    ("gulpfile.js", '', "#cc3e44"),
    ("gulpfile.ts", '', "#cc3e44"),
    ("hypridle.conf", '', "#00aaae"),
    ("hyprland.conf", '', "#00aaae"),
    ("hyprlandd.conf", '', "#00aaae"),
    ("hyprlock.conf", '', "#00aaae"),
    ("hyprpaper.conf", '', "#00aaae"),
    ("hyprsunset.conf", '', "#00aaae"),
    ("i18n.config.js", '󰗊', "#7986cb"),
    ("i18n.config.ts", '󰗊', "#7986cb"),
    ("i3blocks.conf", '', "#e8ebee"),
    ("i3status.conf", '', "#e8ebee"),
    ("index.theme", '', "#2db96f"),
    ("ionic.config.json", '', "#4f8ff7"),
    ("Jenkinsfile", '', "#d33833"),
    ("justfile", '', "#6d8086"),
    ("kalgebrarc", '', "#1c99f3"),
    ("kdeglobals", '', "#1c99f3"),
    ("kdenlive-layoutsrc", '', "#83b8f2"),
    ("kdenliverc", '', "#83b8f2"),
    ("kritadisplayrc", '', "#f245fb"),
    ("kritarc", '', "#f245fb"),
    ("license", '', "#d0bf41"),
    ("license.md", '', "#d0bf41"),
    ("lxde-rc.xml", '', "#909090"),
    ("lxqt.conf", '', "#0192d3"),
    ("makefile", '', "#6d8086"),
    ("mix.lock", '', "#a074c4"),
    ("mpv.conf", '', "#3b1342"),
    ("next.config.cjs", '', "#ffffff"),
    ("next.config.js", '', "#ffffff"),
    ("next.config.ts", '', "#ffffff"),
    ("node_modules", '', "#e8274b"),
    ("nuxt.config.cjs", '󱄆', "#00c58e"),
    ("nuxt.config.js", '󱄆', "#00c58e"),
    ("nuxt.config.mjs", '󱄆', "#00c58e"),
    ("nuxt.config.ts", '󱄆', "#00c58e"),
    ("package-lock.json", '', "#7a0d21"),
    ("package.json", '', "#e8274b"),
    ("PKGBUILD", '', "#0f94d2"),
    ("platformio.ini", '', "#f6822b"),
    ("playwright.config.cjs", '', "#2fad33"),
    ("playwright.config.cts", '', "#2fad33"),
    ("playwright.config.js", '', "#2fad33"),
    ("playwright.config.mjs", '', "#2fad33"),
    ("playwright.config.mts", '', "#2fad33"),
    ("playwright.config.ts", '', "#2fad33"),
    ("pnpm-lock.yaml", '', "#f9ad02"),
    ("pnpm-workspace.yaml", '', "#f9ad02"),
    ("pom.xml", '', "#7a0d21"),
    ("prettier.config.cjs", '', "#4285f4"),
    ("prettier.config.js", '', "#4285f4"),
    ("prettier.config.mjs", '', "#4285f4"),
    ("prettier.config.ts", '', "#4285f4"),
    ("prisma.config.mts", '', "#5a67d8"),
    ("prisma.config.ts", '', "#5a67d8"),
    ("procfile", '', "#a074c4"),
    ("PrusaSlicer.ini", '', "#ec6b23"),
    ("PrusaSlicerGcodeViewer.ini", '', "#ec6b23"),
    ("py.typed", '', "#ffbc03"),
    ("QtProject.conf", '', "#40cd52"),
    ("rakefile", '', "#701516"),
    ("readme", '󰂺', "#ededed"),
    ("readme.md", '󰂺', "#ededed"),
    ("rmd", '', "#519aba"),
    ("robots.txt", '󰚩', "#5d7096"),
    ("security", '󰒃', "#bec4c9"),
    ("security.md", '󰒃', "#bec4c9"),
    ("settings.gradle", '', "#005f87"),
    ("svelte.config.js", '', "#ff3e00"),
    ("sxhkdrc", '', "#2f2f2f"),
    ("sym-lib-table", '', "#ffffff"),
    ("tailwind.config.js", '󱏿', "#20c2e3"),
    ("tailwind.config.mjs", '󱏿', "#20c2e3"),
    ("tailwind.config.ts", '󱏿', "#20c2e3"),
    ("tmux.conf", '', "#14ba19"),
    ("tmux.conf.local", '', "#14ba19"),
    ("tsconfig.json", '', "#519aba"),
    ("unlicense", '', "#d0bf41"),
    ("vagrantfile", '', "#1563ff"),
    ("vercel.json", '', "#ffffff"),
    ("vite.config.cjs", '', "#ffa800"),
    ("vite.config.cts", '', "#ffa800"),
    ("vite.config.js", '', "#ffa800"),
    ("vite.config.mjs", '', "#ffa800"),
    ("vite.config.mts", '', "#ffa800"),
    ("vite.config.ts", '', "#ffa800"),
    ("vitest.config.cjs", '', "#739b1b"),
    ("vitest.config.cts", '', "#739b1b"),
    ("vitest.config.js", '', "#739b1b"),
    ("vitest.config.mjs", '', "#739b1b"),
    ("vitest.config.mts", '', "#739b1b"),
    ("vitest.config.ts", '', "#739b1b"),
    ("vlcrc", '󰕼', "#ee7a00"),
    ("webpack", '󰜫', "#519aba"),
    ("weston.ini", '', "#ffbb01"),
    ("workspace", '', "#89e051"),
    ("wrangler.jsonc", '', "#f48120"),
    ("wrangler.toml", '', "#f48120"),
    ("xdph.conf", '', "#00aaae"),
    ("xmobarrc", '', "#fd4d5d"),
    ("xmobarrc.hs", '', "#fd4d5d"),
    ("xmonad.hs", '', "#fd4d5d"),
    ("xorg.conf", '', "#e54d18"),
    ("xsettingsd.conf", '', "#e54d18"),
];

/// File extensions, lowercase and without the dot.
static EXTS: [(&str, char, &str); 485] = [
    ("3gp", '', "#fd971f"),
    ("3mf", '󰆧', "#888888"),
    ("7z", '', "#eca517"),
    ("a", '', "#dcddd6"),
    ("aac", '', "#00afff"),
    ("ada", '', "#599eff"),
    ("adb", '', "#599eff"),
    ("ads", '', "#a074c4"),
    ("ai", '', "#cbcb41"),
    ("aif", '', "#00afff"),
    ("aiff", '', "#00afff"),
    ("android", '', "#34a853"),
    ("ape", '', "#00afff"),
    ("apk", '', "#34a853"),
    ("apl", '', "#24a148"),
    ("app", '', "#9f0500"),
    ("applescript", '', "#6d8085"),
    ("asc", '󰦝', "#576d7f"),
    ("asm", '', "#0091bd"),
    ("ass", '󰨖', "#ffb713"),
    ("astro", '', "#e23f67"),
    ("avif", '', "#a074c4"),
    ("awk", '', "#4d5a5e"),
    ("azcli", '', "#0078d4"),
    ("bak", '󰁯', "#6d8086"),
    ("bash", '', "#89e051"),
    ("bat", '', "#c1f12e"),
    ("bazel", '', "#89e051"),
    ("bib", '󱉟', "#cbcb41"),
    ("bicep", '', "#519aba"),
    ("bicepparam", '', "#9f74b3"),
    ("bin", '', "#9f0500"),
    ("blade.php", '', "#f05340"),
    ("blend", '󰂫', "#ea7600"),
    ("blp", '󰺾', "#5796e2"),
    ("bmp", '', "#a074c4"),
    ("bqn", '', "#24a148"),
    ("brep", '󰻫', "#839463"),
    ("bz", '', "#eca517"),
    ("bz2", '', "#eca517"),
    ("bz3", '', "#eca517"),
    ("bzl", '', "#89e051"),
    ("c", '', "#599eff"),
    ("c++", '', "#f34b7d"),
    ("cache", '', "#ffffff"),
    ("cast", '', "#fd971f"),
    ("cbl", '', "#005ca5"),
    ("cc", '', "#f34b7d"),
    ("ccm", '', "#f34b7d"),
    ("cfc", '', "#01a4ba"),
    ("cfg", '', "#6d8086"),
    ("cfm", '', "#01a4ba"),
    ("cjs", '', "#cbcb41"),
    ("clj", '', "#8dc149"),
    ("cljc", '', "#8dc149"),
    ("cljd", '', "#519aba"),
    ("cljs", '', "#519aba"),
    ("cmake", '', "#dce3eb"),
    ("cob", '', "#005ca5"),
    ("cobol", '', "#005ca5"),
    ("coffee", '', "#cbcb41"),
    ("conda", '', "#43b02a"),
    ("conf", '', "#6d8086"),
    ("config.ru", '', "#701516"),
    ("cow", '󰆚', "#965824"),
    ("cp", '', "#519aba"),
    ("cpp", '', "#519aba"),
    ("cppm", '', "#519aba"),
    ("cpy", '', "#005ca5"),
    ("cr", '', "#c8c8c8"),
    ("crdownload", '', "#44cda8"),
    ("cs", '󰌛', "#596706"),
    ("csh", '', "#4d5a5e"),
    ("cshtml", '󱦗', "#512bd4"),
    ("cson", '', "#cbcb41"),
    ("csproj", '󰪮', "#512bd4"),
    ("css", '', "#663399"),
    ("csv", '', "#89e051"),
    ("cts", '', "#519aba"),
    ("cu", '', "#89e051"),
    ("cue", '󰲹', "#ed95ae"),
    ("cuh", '', "#a074c4"),
    ("cxx", '', "#519aba"),
    ("cxxm", '', "#519aba"),
    ("d", '', "#b03931"),
    ("d.ts", '', "#d59855"),
    ("dart", '', "#03589c"),
    ("db", '', "#dad8d8"),
    ("dconf", '', "#ffffff"),
    ("desktop", '', "#563d7c"),
    ("diff", '', "#41535b"),
    ("dll", '', "#4d2c0b"),
    ("doc", '󰈬', "#185abd"),
    ("Dockerfile", '󰡨', "#458ee6"),
    ("dockerignore", '󰡨', "#458ee6"),
    ("docx", '󰈬', "#185abd"),
    ("dot", '󱁉', "#30638e"),
    ("download", '', "#44cda8"),
    ("drl", '', "#ffafaf"),
    ("dropbox", '', "#0061fe"),
    ("dump", '', "#dad8d8"),
    ("dwg", '󰻫', "#839463"),
    ("dxf", '󰻫', "#839463"),
    ("ebook", '', "#eab16d"),
    ("ebuild", '', "#4c416e"),
    ("edn", '', "#519aba"),
    ("eex", '', "#a074c4"),
    ("ejs", '', "#cbcb41"),
    ("el", '', "#8172be"),
    ("elc", '', "#8172be"),
    ("elf", '', "#9f0500"),
    ("elm", '', "#519aba"),
    ("eln", '', "#8172be"),
    ("env", '', "#faf743"),
    ("eot", '', "#ececec"),
    ("epp", '', "#ffa61a"),
    ("epub", '', "#eab16d"),
    ("erb", '', "#701516"),
    ("erl", '', "#b83998"),
    ("ex", '', "#a074c4"),
    ("exe", '', "#9f0500"),
    ("exs", '', "#a074c4"),
    ("f#", '', "#519aba"),
    ("f3d", '󰻫', "#839463"),
    ("f90", '󱈚', "#734f96"),
    ("fbx", '󰆧', "#888888"),
    ("fcbak", '', "#cb333b"),
    ("fcmacro", '', "#cb333b"),
    ("fcmat", '', "#cb333b"),
    ("fcparam", '', "#cb333b"),
    ("fcscript", '', "#cb333b"),
    ("fcstd", '', "#cb333b"),
    ("fcstd1", '', "#cb333b"),
    ("fctb", '', "#cb333b"),
    ("fctl", '', "#cb333b"),
    ("fdmdownload", '', "#44cda8"),
    ("feature", '', "#00a818"),
    ("fish", '', "#4d5a5e"),
    ("flac", '', "#0075aa"),
    ("flc", '', "#ececec"),
    ("flf", '', "#ececec"),
    ("fnl", '', "#fff3d7"),
    ("fodg", '', "#fffb57"),
    ("fodp", '', "#fe9c45"),
    ("fods", '', "#78fc4e"),
    ("fodt", '', "#2dcbfd"),
    ("frag", '', "#5586a6"),
    ("fs", '', "#519aba"),
    ("fsi", '', "#519aba"),
    ("fsscript", '', "#519aba"),
    ("fsx", '', "#519aba"),
    ("gcode", '󰐫', "#1471ad"),
    ("gd", '', "#6d8086"),
    ("gemspec", '', "#701516"),
    ("geom", '', "#5586a6"),
    ("gif", '', "#a074c4"),
    ("git", '', "#f14c28"),
    ("glb", '', "#ffb13b"),
    ("gleam", '', "#ffaff3"),
    ("glsl", '', "#5586a6"),
    ("gnumakefile", '', "#6d8086"),
    ("go", '', "#00add8"),
    ("godot", '', "#6d8086"),
    ("gpr", '', "#6d8086"),
    ("gql", '', "#e535ab"),
    ("gradle", '', "#005f87"),
    ("graphql", '', "#e535ab"),
    ("gresource", '', "#ffffff"),
    ("gv", '󱁉', "#30638e"),
    ("gz", '', "#eca517"),
    ("h", '', "#a074c4"),
    ("haml", '', "#eaeae1"),
    ("hbs", '', "#f0772b"),
    ("heex", '', "#a074c4"),
    ("hex", '', "#2e63ff"),
    ("hh", '', "#a074c4"),
    ("hpp", '', "#a074c4"),
    ("hrl", '', "#b83998"),
    ("hs", '', "#a074c4"),
    ("htm", '', "#e34c26"),
    ("html", '', "#e44d26"),
    ("http", '', "#008ec7"),
    ("huff", '󰡘', "#4242c7"),
    ("hurl", '', "#ff0288"),
    ("hx", '', "#ea8220"),
    ("hxx", '', "#a074c4"),
    ("ical", '', "#2b2e83"),
    ("icalendar", '', "#2b2e83"),
    ("ico", '', "#cbcb41"),
    ("ics", '', "#2b2e83"),
    ("ifb", '', "#2b2e83"),
    ("ifc", '󰻫', "#839463"),
    ("ige", '󰻫', "#839463"),
    ("iges", '󰻫', "#839463"),
    ("igs", '󰻫', "#839463"),
    ("image", '', "#d0bec8"),
    ("img", '', "#d0bec8"),
    ("import", '', "#ececec"),
    ("info", '', "#ffffcd"),
    ("ini", '', "#6d8086"),
    ("ino", '', "#56b6c2"),
    ("ipynb", '', "#f57d01"),
    ("iso", '', "#d0bec8"),
    ("ixx", '', "#519aba"),
    ("jar", '', "#ffaf67"),
    ("java", '', "#cc3e44"),
    ("jl", '', "#a270ba"),
    ("jpeg", '', "#a074c4"),
    ("jpg", '', "#a074c4"),
    ("js", '', "#cbcb41"),
    ("json", '', "#cbcb41"),
    ("json5", '', "#cbcb41"),
    ("jsonc", '', "#cbcb41"),
    ("jsx", '', "#20c2e3"),
    ("jwmrc", '', "#0078cd"),
    ("jxl", '', "#a074c4"),
    ("kbx", '󰯄', "#737672"),
    ("kdb", '', "#529b34"),
    ("kdbx", '', "#529b34"),
    ("kdenlive", '', "#83b8f2"),
    ("kdenlivetitle", '', "#83b8f2"),
    ("kicad_dru", '', "#ffffff"),
    ("kicad_mod", '', "#ffffff"),
    ("kicad_pcb", '', "#ffffff"),
    ("kicad_prl", '', "#ffffff"),
    ("kicad_pro", '', "#ffffff"),
    ("kicad_sch", '', "#ffffff"),
    ("kicad_sym", '', "#ffffff"),
    ("kicad_wks", '', "#ffffff"),
    ("ko", '', "#dcddd6"),
    ("kpp", '', "#f245fb"),
    ("kra", '', "#f245fb"),
    ("krz", '', "#f245fb"),
    ("ksh", '', "#4d5a5e"),
    ("kt", '', "#7f52ff"),
    ("kts", '', "#7f52ff"),
    ("lck", '', "#bbbbbb"),
    ("leex", '', "#a074c4"),
    ("less", '', "#563d7c"),
    ("lff", '', "#ececec"),
    ("lhs", '', "#a074c4"),
    ("lib", '', "#4d2c0b"),
    ("license", '', "#cbcb41"),
    ("liquid", '', "#95bf47"),
    ("lock", '', "#bbbbbb"),
    ("log", '󰌱', "#dddddd"),
    ("lrc", '󰨖', "#ffb713"),
    ("lua", '', "#51a0cf"),
    ("luac", '', "#51a0cf"),
    ("luau", '', "#00a2ff"),
    ("m", '', "#599eff"),
    ("m3u", '󰲹', "#ed95ae"),
    ("m3u8", '󰲹', "#ed95ae"),
    ("m4a", '', "#00afff"),
    ("m4v", '', "#fd971f"),
    ("magnet", '', "#a51b16"),
    ("makefile", '', "#6d8086"),
    ("markdown", '', "#dddddd"),
    ("material", '', "#b83998"),
    ("md", '', "#dddddd"),
    ("md5", '󰕥', "#8c86af"),
    ("mdx", '', "#519aba"),
    ("mint", '󰌪', "#87c095"),
    ("mjs", '', "#f1e05a"),
    ("mk", '', "#6d8086"),
    ("mkv", '', "#fd971f"),
    ("ml", '', "#e37933"),
    ("mli", '', "#e37933"),
    ("mm", '', "#519aba"),
    ("mo", '', "#9772fb"),
    ("mobi", '', "#eab16d"),
    ("mojo", '', "#ff4c1f"),
    ("mov", '', "#fd971f"),
    ("mp3", '', "#00afff"),
    ("mp4", '', "#fd971f"),
    ("mpp", '', "#519aba"),
    ("msf", '', "#137be1"),
    ("mts", '', "#519aba"),
    ("mustache", '', "#e37933"),
    ("nfo", '', "#ffffcd"),
    ("nim", '', "#f3d400"),
    ("nix", '', "#7ebae4"),
    ("norg", '', "#4878be"),
    ("nswag", '', "#85ea2d"),
    ("nu", '', "#3aa675"),
    ("o", '', "#9f0500"),
    ("obj", '󰆧', "#888888"),
    ("odf", '', "#ff5a96"),
    ("odg", '', "#fffb57"),
    ("odin", '󰟢', "#3882d2"),
    ("odp", '', "#fe9c45"),
    ("ods", '', "#78fc4e"),
    ("odt", '', "#2dcbfd"),
    ("oga", '', "#0075aa"),
    ("ogg", '', "#0075aa"),
    ("ogv", '', "#fd971f"),
    ("ogx", '', "#fd971f"),
    ("opus", '', "#0075aa"),
    ("org", '', "#77aa99"),
    ("otf", '', "#ececec"),
    ("out", '', "#9f0500"),
    ("part", '', "#44cda8"),
    ("patch", '', "#41535b"),
    ("pck", '', "#6d8086"),
    ("pcm", '', "#0075aa"),
    ("pdf", '', "#b30b00"),
    ("php", '', "#a074c4"),
    ("pl", '', "#519aba"),
    ("pls", '󰲹', "#ed95ae"),
    ("ply", '󰆧', "#888888"),
    ("pm", '', "#519aba"),
    ("png", '', "#a074c4"),
    ("po", '', "#2596be"),
    ("pot", '', "#2596be"),
    ("pp", '', "#ffa61a"),
    ("ppt", '󰈧', "#cb4a32"),
    ("pptx", '󰈧', "#cb4a32"),
    ("prisma", '', "#5a67d8"),
    ("pro", '', "#e4b854"),
    ("ps1", '󰨊', "#4273ca"),
    ("psb", '', "#519aba"),
    ("psd", '', "#519aba"),
    ("psd1", '󰨊', "#6975c4"),
    ("psm1", '󰨊', "#6975c4"),
    ("pub", '󰷖', "#e3c58e"),
    ("pxd", '', "#5aa7e4"),
    ("pxi", '', "#5aa7e4"),
    ("py", '', "#ffbc03"),
    ("pyc", '', "#ffe291"),
    ("pyd", '', "#ffe291"),
    ("pyi", '', "#ffbc03"),
    ("pyo", '', "#ffe291"),
    ("pyw", '', "#5aa7e4"),
    ("pyx", '', "#5aa7e4"),
    ("qm", '', "#2596be"),
    ("qml", '', "#40cd52"),
    ("qrc", '', "#40cd52"),
    ("qss", '', "#40cd52"),
    ("query", '', "#90a850"),
    ("R", '󰟔', "#2266ba"),
    ("r", '󰟔', "#2266ba"),
    ("rake", '', "#701516"),
    ("rar", '', "#eca517"),
    ("rasi", '', "#cbcb41"),
    ("razor", '󱦘', "#512bd4"),
    ("rb", '', "#701516"),
    ("res", '', "#cc3e44"),
    ("resi", '', "#f55385"),
    ("rlib", '', "#dea584"),
    ("rmd", '', "#519aba"),
    ("rproj", '󰗆', "#358a5b"),
    ("rs", '', "#dea584"),
    ("rss", '', "#fb9d3b"),
    ("s", '', "#0071c5"),
    ("sass", '', "#f55385"),
    ("sbt", '', "#cc3e44"),
    ("sc", '', "#cc3e44"),
    ("scad", '', "#f9d72c"),
    ("scala", '', "#cc3e44"),
    ("scm", '󰘧', "#eeeeee"),
    ("scss", '', "#f55385"),
    ("sh", '', "#4d5a5e"),
    ("sha1", '󰕥', "#8c86af"),
    ("sha224", '󰕥', "#8c86af"),
    ("sha256", '󰕥', "#8c86af"),
    ("sha384", '󰕥', "#8c86af"),
    ("sha512", '󰕥', "#8c86af"),
    ("sig", '󰘧', "#e37933"),
    ("signature", '󰘧', "#e37933"),
    ("skp", '󰻫', "#839463"),
    ("sldasm", '󰻫', "#839463"),
    ("sldprt", '󰻫', "#839463"),
    ("slim", '', "#e34c26"),
    ("sln", '', "#854cc7"),
    ("slnx", '', "#854cc7"),
    ("slvs", '󰻫', "#839463"),
    ("sml", '󰘧', "#e37933"),
    ("so", '', "#dcddd6"),
    ("sol", '', "#519aba"),
    ("spec.js", '', "#cbcb41"),
    ("spec.jsx", '', "#20c2e3"),
    ("spec.ts", '', "#519aba"),
    ("spec.tsx", '', "#1354bf"),
    ("spx", '', "#0075aa"),
    ("sql", '', "#dad8d8"),
    ("sqlite", '', "#dad8d8"),
    ("sqlite3", '', "#dad8d8"),
    ("srt", '󰨖', "#ffb713"),
    ("ssa", '󰨖', "#ffb713"),
    ("ste", '󰻫', "#839463"),
    ("step", '󰻫', "#839463"),
    ("stl", '󰆧', "#888888"),
    ("stories.js", '', "#ff4785"),
    ("stories.jsx", '', "#ff4785"),
    ("stories.mjs", '', "#ff4785"),
    ("stories.svelte", '', "#ff4785"),
    ("stories.ts", '', "#ff4785"),
    ("stories.tsx", '', "#ff4785"),
    ("stories.vue", '', "#ff4785"),
    ("stp", '󰻫', "#839463"),
    ("strings", '', "#2596be"),
    ("styl", '', "#8dc149"),
    ("sub", '󰨖', "#ffb713"),
    ("sublime", '', "#e37933"),
    ("suo", '', "#854cc7"),
    ("sv", '󰍛', "#019833"),
    ("svelte", '', "#ff3e00"),
    ("svg", '󰜡', "#ffb13b"),
    ("svgz", '󰜡', "#ffb13b"),
    ("svh", '󰍛', "#019833"),
    ("swift", '', "#e37933"),
    ("t", '', "#519aba"),
    ("tbc", '󰛓', "#1e5cb3"),
    ("tcl", '󰛓', "#1e5cb3"),
    ("templ", '', "#dbbd30"),
    ("terminal", '', "#31b53e"),
    ("test.js", '', "#cbcb41"),
    ("test.jsx", '', "#20c2e3"),
    ("test.ts", '', "#519aba"),
    ("test.tsx", '', "#1354bf"),
    ("tex", '', "#3d6117"),
    ("tf", '', "#5f43e9"),
    ("tfvars", '', "#5f43e9"),
    ("tgz", '', "#eca517"),
    ("tmpl", '', "#dbbd30"),
    ("tmux", '', "#14ba19"),
    ("toml", '', "#9c4221"),
    ("torrent", '', "#44cda8"),
    ("tres", '', "#6d8086"),
    ("ts", '', "#519aba"),
    ("tscn", '', "#6d8086"),
    ("tsconfig", '', "#ff8700"),
    ("tsx", '', "#1354bf"),
    ("ttf", '', "#ececec"),
    ("twig", '', "#8dc149"),
    ("txt", '󰈙', "#89e051"),
    ("txz", '', "#eca517"),
    ("typ", '', "#0dbcc0"),
    ("typoscript", '', "#ff8700"),
    ("ui", '', "#015bf0"),
    ("v", '󰍛', "#019833"),
    ("vala", '', "#7b3db9"),
    ("vert", '', "#5586a6"),
    ("vh", '󰍛', "#019833"),
    ("vhd", '󰍛', "#019833"),
    ("vhdl", '󰍛', "#019833"),
    ("vi", '', "#fec60a"),
    ("vim", '', "#019833"),
    ("vsh", '', "#5d87bf"),
    ("vsix", '', "#854cc7"),
    ("vue", '', "#8dc149"),
    ("wasm", '', "#5c4cdb"),
    ("wav", '', "#00afff"),
    ("webm", '', "#fd971f"),
    ("webmanifest", '', "#f1e05a"),
    ("webp", '', "#a074c4"),
    ("webpack", '󰜫', "#519aba"),
    ("wma", '', "#00afff"),
    ("wmv", '', "#fd971f"),
    ("woff", '', "#ececec"),
    ("woff2", '', "#ececec"),
    ("wrl", '󰆧', "#888888"),
    ("wrz", '󰆧', "#888888"),
    ("wv", '', "#00afff"),
    ("wvc", '', "#00afff"),
    ("x", '', "#599eff"),
    ("xaml", '󰙳', "#512bd4"),
    ("xcf", '', "#635b46"),
    ("xcplayground", '', "#e37933"),
    ("xcstrings", '', "#2596be"),
    ("xls", '󰈛', "#207245"),
    ("xlsx", '󰈛', "#207245"),
    ("xm", '', "#519aba"),
    ("xml", '󰗀', "#e37933"),
    ("xpi", '', "#ff1b01"),
    ("xslt", '󰗀', "#33a9dc"),
    ("xul", '', "#e37933"),
    ("xz", '', "#eca517"),
    ("yaml", '', "#6d8086"),
    ("yml", '', "#6d8086"),
    ("zig", '', "#f69a1b"),
    ("zip", '', "#eca517"),
    ("zsh", '', "#89e051"),
    ("zst", '', "#eca517"),
    ("🔥", '', "#ff4c1f"),
];

/// An icon as shown in the listing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Icon {
    pub glyph: char,
    /// `#rrggbb` from Yazi's theme.
    pub color: &'static str,
}

/// Resolve the icon for an entry.
///
/// `name` is the entry name, which is cheaper than re-deriving it from the path
/// and also works for the synthetic `..` row.
pub fn resolve(name: &str, path: &Path, is_dir: bool, is_parent: bool) -> Icon {
    if is_parent {
        return Icon {
            glyph: PARENT,
            color: DEFAULT_COLOR,
        };
    }

    if is_dir {
        let lower = name.to_lowercase();
        if let Some(found) = DIRS
            .iter()
            .find(|(key, _, _)| key.eq_ignore_ascii_case(&lower))
        {
            return Icon {
                glyph: found.1,
                color: found.2,
            };
        }
        return Icon {
            glyph: FOLDER,
            color: DEFAULT_COLOR,
        };
    }

    if let Some(found) = FILES
        .iter()
        .find(|(key, _, _)| key.eq_ignore_ascii_case(name))
    {
        return Icon {
            glyph: found.1,
            color: found.2,
        };
    }

    if let Some(extension) = path.extension().and_then(|ext| ext.to_str()) {
        let lower = extension.to_lowercase();
        if let Some(found) = EXTS.iter().find(|(key, _, _)| *key == lower) {
            return Icon {
                glyph: found.1,
                color: found.2,
            };
        }
    }

    Icon {
        glyph: FILE,
        color: DEFAULT_COLOR,
    }
}

/// Parse a `#rrggbb` colour from the theme tables.
pub fn color(hex: &str) -> ratatui::style::Color {
    let hex = hex.trim_start_matches('#');
    if hex.len() != 6 {
        return ratatui::style::Color::Gray;
    }
    let channel =
        |range: std::ops::Range<usize>| u8::from_str_radix(&hex[range], 16).unwrap_or(128);
    ratatui::style::Color::Rgb(channel(0..2), channel(2..4), channel(4..6))
}

/// Every icon is this many cells wide.
pub const ICON_WIDTH: usize = 9;
/// Every icon is this many cells tall.
pub const ICON_HEIGHT: usize = 6;

/// Block art for one group, used by the thumbnail grid.
pub fn art(group: FileGroup) -> &'static [&'static str; ICON_HEIGHT] {
    match group {
        FileGroup::Directory => &FOLDER_ART,
        FileGroup::Image => &IMAGE_ART,
        FileGroup::Video => &VIDEO_ART,
        FileGroup::Audio => &AUDIO_ART,
        FileGroup::Text => &TEXT_ART,
        FileGroup::Archive => &ARCHIVE_ART,
        FileGroup::Temporary => &TEMPORARY_ART,
        FileGroup::Model => &MODEL_ART,
        FileGroup::Code => &CODE_ART,
        FileGroup::Other => &OTHER_ART,
    }
}

/// Single-character fallback when a tile is too small for the artwork.
pub fn art_glyph(group: FileGroup) -> char {
    match group {
        FileGroup::Directory => '▸',
        FileGroup::Image => '▣',
        FileGroup::Video => '▶',
        FileGroup::Audio => '♪',
        FileGroup::Text => '≡',
        FileGroup::Archive => '▤',
        FileGroup::Temporary => '·',
        FileGroup::Model => '◈',
        FileGroup::Code => '‹',
        FileGroup::Other => '▪',
    }
}

const FOLDER_ART: [&str; ICON_HEIGHT] = [
    "┌───────┐",
    "│█▄     │",
    "│██████ │",
    "│██████ │",
    "│██████ │",
    "└───────┘",
];

const IMAGE_ART: [&str; ICON_HEIGHT] = [
    "┌───────┐",
    "│ ▄▄▄▄▄ │",
    "│ █   █ │",
    "│ █ ▄ █ │",
    "│ ▀▀▀▀▀ │",
    "└───────┘",
];

const VIDEO_ART: [&str; ICON_HEIGHT] = [
    "┌───────┐",
    "│   █   │",
    "│   ██  │",
    "│   ███ │",
    "│   ██  │",
    "└── █───┘",
];

const AUDIO_ART: [&str; ICON_HEIGHT] = [
    "┌───────┐",
    "│    ██ │",
    "│    ██ │",
    "│    ██ │",
    "│  ████ │",
    "└─████──┘",
];

const TEXT_ART: [&str; ICON_HEIGHT] = [
    "┌───────┐",
    "│▒▒▒▒▒▒▒│",
    "│▒▒▒▒▒▒▒│",
    "│▒▒▒▒▒  │",
    "│▒▒▒▒▒▒▒│",
    "└───────┘",
];

const ARCHIVE_ART: [&str; ICON_HEIGHT] = [
    "┌───────┐",
    "│   █   │",
    "│  █ █  │",
    "│   █   │",
    "│  █ █  │",
    "└───────┘",
];

const TEMPORARY_ART: [&str; ICON_HEIGHT] = [
    "┌───────┐",
    "│       │",
    "│  ░░░  │",
    "│  ░░░  │",
    "│       │",
    "└───────┘",
];

const MODEL_ART: [&str; ICON_HEIGHT] = [
    "┌───────┐",
    "│  ▄▄▄  │",
    "│ █▀▀▀█ │",
    "│ █   █ │",
    "│ ▀▄▄▄▀ │",
    "└───────┘",
];

const CODE_ART: [&str; ICON_HEIGHT] = [
    "┌───────┐",
    "│  ███  │",
    "│ █   █ │",
    "│ █   █ │",
    "│  ███  │",
    "└───────┘",
];

const OTHER_ART: [&str; ICON_HEIGHT] = [
    "┌───────┐",
    "│       │",
    "│   ▪   │",
    "│       │",
    "│       │",
    "└───────┘",
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn every_art_icon_has_the_same_shape() {
        for group in FileGroup::ALL {
            let art = art(group);
            assert_eq!(art.len(), ICON_HEIGHT, "{group:?} has the wrong height");
            for line in art {
                assert_eq!(
                    line.width(),
                    ICON_WIDTH,
                    "{group:?} line has the wrong width: {line:?}"
                );
            }
        }
    }

    #[test]
    fn folders_use_the_documents_glyph_and_a_known_colour() {
        let icon = resolve("Documents", Path::new("/home/me/Documents"), true, false);
        assert_eq!(icon.glyph, '\u{f401}');
        assert_eq!(icon.color, "#00bcd4");

        let plain = resolve(
            "random-folder",
            Path::new("/home/me/random-folder"),
            true,
            false,
        );
        assert_eq!(plain.glyph, FOLDER);
    }

    #[test]
    fn extensions_and_file_names_are_matched() {
        let rust = resolve("main.rs", Path::new("/src/main.rs"), false, false);
        assert_eq!(rust.glyph, '\u{e68b}', "rust files get the Rust glyph");

        let image = resolve("photo.PNG", Path::new("/photos/photo.PNG"), false, false);
        assert_eq!(
            image.glyph, '\u{e60d}',
            "extensions match case-insensitively"
        );

        let gitignore = resolve(".gitignore", Path::new("/repo/.gitignore"), false, false);
        assert_eq!(gitignore.color, "#f54d27", "file names beat extensions");

        let unknown = resolve("blob", PathBuf::from("/tmp/blob").as_path(), false, false);
        assert_eq!(unknown.glyph, FILE);
        assert_eq!(unknown.color, DEFAULT_COLOR);
    }

    #[test]
    fn the_parent_row_has_its_own_glyph() {
        let icon = resolve("..", Path::new("/tmp"), true, true);
        assert_eq!(icon.glyph, PARENT);
    }

    #[test]
    fn hex_colours_are_parsed() {
        assert_eq!(color("#00bcd4"), ratatui::style::Color::Rgb(0, 188, 212));
        assert_eq!(color("00bcd4"), ratatui::style::Color::Rgb(0, 188, 212));
        assert_eq!(color("nope"), ratatui::style::Color::Gray);
    }
}
