{ lib
, rustPlatform
, binutils
, mold
, openssl
, pkg-config
}:

let
  manifest = (lib.importTOML ../../Cargo.toml).package;
in
rustPlatform.buildRustPackage {
  pname = manifest.name;
  version = manifest.version;

  # TODO: upgrade to lib.fileset as cleanSource is including many irrelevent
  # files for the build (many *.md files, .git* files, & so on).
  src = lib.cleanSource ../..;

  cargoLock.lockFile = ../../Cargo.lock;

  nativeBuildInputs = [
    binutils
    mold
    pkg-config
  ];

  buildInputs = [
    openssl
  ];

  buildFeatures = [
    "acp"
    "memory"
    "multithread"
  ];

  # TODO: there needs to be a list of tests that can vs. can’t run in the Nix
  # sandbox
  doCheck = false;

  meta = {
    description = manifest.description;
    license = lib.licenses.gpl3Only;
    homepage = manifest.homepage;
    mainProgram = "zerostack";
    platforms = with lib.platforms; linux ++ darwin;
  };
}
