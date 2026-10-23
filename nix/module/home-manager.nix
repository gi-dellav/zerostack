{ config
, pkgs
, lib
, ...
}:

let
  cfg = config.programs.zerostack;

  mkPrompt = pname: pval: {
    name = "zerostack/prompts/${pname}.md";
    value =
      if lib.isPath pval then
        { source = pval; }
      else
        { text = pval; };
  };

  mkTheme = tname: tval: {
    name = "zerostack/themes/${tname}.json";
    value =
      if lib.isPath tval then
        { source = tval; }
      else
        { text = lib.toJSON tval; };
  };

  tomlFormat = pkgs.formats.toml { };
in
{
  options.programs.zerostack = {
    enable = lib.mkEnableOption "zerostack TUI";

    package = lib.mkPackageOption pkgs "zerostack" { };

    settings = lib.mkOption {
      type = lib.types.toml;
      description = "TOML configuration for zerostack; see: https://gi-dellav.github.io/zerostack/CONFIG.html";
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
      description = "Prompt files (in Markdown)";
      default = { };
    };

    themes = lib.mkOption {
      type = with lib.types; attrsOf (either json path);
      description = "Theme files or attributes";
      default = { };
    };
  };

  config = lib.mkIf cfg.enable {
    home.packages = [ cfg.package ];

    xdg.dataFile =
      lib.mapAttrs' mkPrompt cfg.prompts
      // lib.mapAttrs' mkTheme cfg.themes;

    xdg.configFile = lib.optionalAttrs (cfg.settings != { }) {
      "zerostack/config.toml".source =
        tomlFormat.generate "zerostack-config" cfg.settings;
    };
  };
}
