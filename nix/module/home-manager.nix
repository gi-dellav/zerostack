{
  config,
  pkgs,
  lib,
  ...
}:

let
  inherit (pkgs.stdenv.hostPlatform) isDarwin;

  cfg = config.programs.zerostack;

  configDir = "${lib.optionalString isDarwin "Library/Application Support/"}zerostack";

  mkPrompt = pname: pval: {
    name = "${configDir}/prompts/${pname}.md";
    value = if lib.isString pval then { text = pval; } else { source = pval; };
  };

  mkTheme = tname: tval: {
    name = "${configDir}/themes/${tname}.json";
    value = if lib.isPath tval then { source = tval; } else { text = lib.toJSON tval; };
  };

  dataFiles = lib.mapAttrs' mkPrompt cfg.prompts // lib.mapAttrs' mkTheme cfg.themes;

  tomlFormat = pkgs.formats.toml { };
in
{
  options.programs.zerostack = {
    enable = lib.mkEnableOption "zerostack TUI";

    package = lib.mkPackageOption pkgs "zerostack" { };

    settings = lib.mkOption {
      type = lib.types.toml;
      description = ''
        TOML configuration written to {file}`$HOME/Library/Application
        Support/zerostack/config.toml` on Darwin (the `dirs` crate ignores
        XDG variables there) else
        {file}`$XDG_CONFIG_HOME/zerostack/config.toml`; see:
        https://gi-dellav.github.io/zerostack/CONFIG.html
      '';
      default = { };
      example = {
        provider = "openrouter";
        model = "deepseek/deepseek-v4-flash";
        max_tokens = 16384;
        compact_enabled = true;
        mid_turn_compact_threshold = 0.80;
        default_prompt = "ask";
      };
    };

    prompts = lib.mkOption {
      type = with lib.types; attrsOf (either str path);
      description = ''
        Prompt files, or text in Markdown, written to
        {file}`$HOME/Library/Application Support/zerostack/prompts/` on
        Darwin else {file}`$XDG_DATA_HOME/zerostack/prompts/`
      '';
      default = { };
    };

    themes = lib.mkOption {
      type = with lib.types; attrsOf (either json path);
      description = ''
        Theme files or attributes written to
        {file}`$HOME/Library/Application Support/zerostack/themes/` on
        Darwin else {file}`$XDG_DATA_HOME/zerostack/themes/`
      '';
      default = { };
    };
  };

  config = lib.mkIf cfg.enable {
    home.packages = [ cfg.package ];

    xdg.dataFile = lib.mkIf (!isDarwin) dataFiles;
    xdg.configFile = lib.optionalAttrs (!isDarwin && cfg.settings != { }) {
      "zerostack/config.toml".source = tomlFormat.generate "zerostack-config" cfg.settings;
    };

    home.file = lib.mkIf isDarwin (
      dataFiles
      // lib.optionalAttrs (cfg.settings != { }) {
        "${configDir}/config.toml".source =
          let
            warnCond =
              cfg.settings ? api_keys
              && (
                let
                  inherit (cfg.settings) api_keys;
                in
                api_keys != { }
                # This is effectively empty
                && api_keys != { openai = "sk-no-key-required"; }
              );
          in
          lib.warnIf warnCond "zerostack: API keys will stored with home-manager be world-readable" (
            tomlFormat.generate "zerostack-config" cfg.settings
          );
      }
    );
  };
}
