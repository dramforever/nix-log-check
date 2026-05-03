{
  lib,
  rustPlatform,
  versionSuffix ? "",
}:

let
  cargoToml = builtins.fromTOML (builtins.readFile ../Cargo.toml);
in

rustPlatform.buildRustPackage {
  pname = cargoToml.package.name;
  version = cargoToml.package.version + versionSuffix;

  src = lib.cleanSource ../.;
  cargoLock.lockFile = ../Cargo.lock;

  env.NIX_LOG_CHECK_VERSION_SUFFIX = versionSuffix;
}
