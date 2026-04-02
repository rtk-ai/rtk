# Home-manager module for RTK (Rust Token Killer).
#
# Usage: import rtk.homeManagerModules.default, then:
#   programs.rtk = {
#     enable = true;
#     claudeCode.enable = true;
#     opencode.enable = false;
#   };
rtkPackages: {
  config,
  lib,
  pkgs,
  ...
}: let
  cfg = config.programs.rtk;
  rtkPkg = rtkPackages.${pkgs.stdenv.hostPlatform.system}.default;

  manifest = pkgs.runCommand "rtk-config" {
    nativeBuildInputs = [rtkPkg pkgs.jq];
  } ''
    ${rtkPkg}/bin/rtk init --dry-run --json \
      ${lib.optionalString cfg.opencode.enable "--opencode"} \
      ${lib.optionalString (cfg.claudeCode.baseClaude != null) "--base-claude-md ${cfg.claudeCode.baseClaude}"} \
      ${lib.optionalString (cfg.claudeCode.baseSettings != null) "--base-settings ${cfg.claudeCode.baseSettings}"} \
      > manifest.json

    count=$(jq '.files | length' manifest.json)
    for i in $(seq 0 $((count - 1))); do
      relpath=$(jq -r ".files[$i].path" manifest.json)
      mode=$(jq -r ".files[$i].mode" manifest.json)
      mkdir -p "$out/$(dirname "$relpath")"
      jq -j ".files[$i].content" manifest.json > "$out/$relpath"
      chmod "$mode" "$out/$relpath"
    done
  '';
in {
  options.programs.rtk = {
    enable = lib.mkEnableOption "RTK (Rust Token Killer) token compression";

    package = lib.mkOption {
      type = lib.types.package;
      default = rtkPkg;
      description = "The RTK package to install.";
    };

    claudeCode = {
      enable = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = "Configure RTK hooks for Claude Code.";
      };

      baseClaude = lib.mkOption {
        type = lib.types.nullOr lib.types.path;
        default = null;
        description = "Seed CLAUDE.md content from this file. RTK appends @RTK.md to it.";
      };

      baseSettings = lib.mkOption {
        type = lib.types.nullOr lib.types.path;
        default = null;
        description = "Seed settings.json from this file. RTK merges the hook entry into it.";
      };
    };

    opencode = {
      enable = lib.mkOption {
        type = lib.types.bool;
        default = false;
        description = "Install RTK plugin for OpenCode.";
      };
    };
  };

  config = lib.mkIf cfg.enable {
    home.packages = [cfg.package];

    home.file = lib.mkIf cfg.claudeCode.enable {
      ".claude/CLAUDE.md".source = "${manifest}/.claude/CLAUDE.md";
      ".claude/settings.json".source = "${manifest}/.claude/settings.json";
      ".claude/RTK.md".source = "${manifest}/.claude/RTK.md";
    };
  };
}
