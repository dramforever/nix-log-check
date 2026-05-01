{
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      eachSystem = nixpkgs.lib.genAttrs [
        "x86_64-linux"
        "aarch64-linux"
      ];
    in
    {
      devShells = eachSystem (system: {
        default = nixpkgs.legacyPackages.${system}.callPackage ./nix/dev.nix { };
      });

      packages = eachSystem (system: {
        default = nixpkgs.legacyPackages.${system}.callPackage ./nix/package.nix {
          versionSuffix =
            with builtins;
            if self.sourceInfo ? lastModifiedDate && self.sourceInfo ? shortRev then
              "-${substring 0 8 self.sourceInfo.lastModifiedDate}-g${self.sourceInfo.shortRev}"
            else
              "";
        };
      });
    };
}
