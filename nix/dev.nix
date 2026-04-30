{
  mkShell,
  cargo,
  rustc,
  rustfmt,
  rustPlatform,
}:

mkShell {
  buildInputs = [
    cargo
    rustc
    rustfmt
  ];
  env.RUST_SRC_PATH = rustPlatform.rustLibSrc;
}
