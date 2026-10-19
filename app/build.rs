//! Compiles pixl's UI, everything `lumen.toml` names under `src/` with its
//! includes and imports, into the artifact the executable carries.

fn main() {
    lumenui_build::compile();
}
