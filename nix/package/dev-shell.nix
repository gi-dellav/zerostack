{ mkShell
, clippy
, rust-analyzer
, rustfmt
, zerostack
}:

mkShell {
  inputsFrom = [ zerostack ];

  buildInputs = [
    clippy
    rust-analyzer
    rustfmt
  ];
}
