# NixOS module for the org-wiki-web service.
# Exported from the flake as nixosModules.web.
#
# Minimal usage (defaults to Unix domain socket):
#
#   inputs.org-wiki.nixosModules.web
#
#   services.org-wiki-web = {
#     enable = true;
#     contentRepo = "/var/lib/org-wiki-web/content";
#     oidcIssuer = "https://authelia.example.com";
#     oidcClientId = "wiki";
#     oidcClientSecretFile = config.age.secrets.wiki-oidc-client-secret.path;
#     baseUrl = "https://wiki.example.com";
#   };
#
# To use TCP instead:
#
#   services.org-wiki-web = {
#     enable = true;
#     socket = null;
#     port   = 8080;
#     ...
#   };
#
# To reference the socket from a reverse proxy (e.g. nginx):
#
#   locations."/".proxyPass =
#     "http://unix:${config.services.org-wiki-web.socket}";
#
# Note: when using socket mode the reverse proxy user must be a member of
# the service group (cfg.group) so it can connect to the socket.
{self}: {
  config,
  lib,
  pkgs,
  ...
}: let
  cfg = config.services.org-wiki-web;
in {
  options.services.org-wiki-web = {
    enable = lib.mkEnableOption "org-wiki-web web service";

    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.web;
      defaultText = lib.literalExpression "self.packages.\${system}.web";
      description = "Package providing the service binary.";
    };

    socket = lib.mkOption {
      type = lib.types.nullOr lib.types.path;
      default = "/run/org-wiki-web/org-wiki-web.sock";
      description = ''
        Path for the Unix domain socket used by the service.  When set,
        systemd socket activation is used and the host/port options are
        ignored.  Set to null to use TCP instead.

        Other services (e.g. nginx) that proxy to this socket must be
        members of the service group to connect.
      '';
    };

    host = lib.mkOption {
      type = lib.types.str;
      default = "127.0.0.1";
      description = "IP address to bind to.  Ignored when socket is set.";
    };

    port = lib.mkOption {
      type = lib.types.port;
      default = 3000;
      description = "TCP port to listen on.  Ignored when socket is set.";
    };

    logLevel = lib.mkOption {
      type = lib.types.enum ["trace" "debug" "info" "warn" "error"];
      default = "info";
      description = "Tracing log verbosity level.";
    };

    logFormat = lib.mkOption {
      type = lib.types.enum ["text" "json"];
      default = "json";
      description = ''
        Log output format.  Use "text" for human-readable local logs and
        "json" for structured logs consumed by a log aggregator.
      '';
    };

    frontendPath = lib.mkOption {
      type = lib.types.str;
      default = "${cfg.package}/share/org-wiki-web/frontend";
      defaultText =
        lib.literalExpression
        ''"''${cfg.package}/share/org-wiki-web/frontend"'';
      description = "Path to compiled frontend static assets.";
    };

    user = lib.mkOption {
      type = lib.types.str;
      default = "org-wiki-web";
      description = "System user account the service runs as.";
    };

    group = lib.mkOption {
      type = lib.types.str;
      default = "org-wiki-web";
      description = "System group the service runs as.";
    };

    # ── wiki content ──────────────────────────────────────────────────────

    formatOnSave = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = ''
        Run the in-process org-fmt formatter on each page before
        committing.  Set to false to commit user input verbatim.
      '';
    };

    contentRepo = lib.mkOption {
      type = lib.types.str;
      default = "/var/lib/org-wiki-web/content";
      description = "Path to the git repository holding the org-mode wiki content.";
    };

    contentRemote = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      description = "Git remote name to push to after each save.  Null to disable push.";
    };

    cacheDir = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      description = "Directory for cached HTML fragments.  Null to disable caching.";
    };

    siteTitle = lib.mkOption {
      type = lib.types.str;
      default = "Org Wiki";
      description = "Human-readable site name shown in the HTML header.";
    };

    # ── git commit identity ───────────────────────────────────────────────

    commitAuthorName = lib.mkOption {
      type = lib.types.str;
      default = "Org Wiki";
      description = "Name used as the git Author on server-side commits.";
    };

    commitAuthorEmail = lib.mkOption {
      type = lib.types.str;
      default = "wiki@localhost";
      description = "Email used as the git Author on server-side commits.";
    };

    # ── OIDC ─────────────────────────────────────────────────────────────

    oidcIssuer = lib.mkOption {
      type = lib.types.str;
      description = "OIDC provider issuer URL (must expose /.well-known/openid-configuration).";
    };

    oidcClientId = lib.mkOption {
      type = lib.types.str;
      description = "OAuth2 client ID registered with the OIDC provider.";
    };

    oidcClientSecretFile = lib.mkOption {
      type = lib.types.path;
      description = "Path to a file containing the OAuth2 client secret.";
    };

    baseUrl = lib.mkOption {
      type = lib.types.str;
      description = "Public base URL of this org-wiki instance (used to build the OIDC redirect URI).";
    };

    # ── webhook ───────────────────────────────────────────────────────────

    webhookSecretFile = lib.mkOption {
      type = lib.types.nullOr lib.types.path;
      default = null;
      description = "Path to a file containing the webhook HMAC-SHA256 shared secret.  Null to accept all requests without verification.";
    };

    # ── SSH deploy key ────────────────────────────────────────────────────

    sshKeyFile = lib.mkOption {
      type = lib.types.nullOr lib.types.path;
      default = null;
      description = "Path to an SSH private key used for pushing to the git remote.  Null to use the default SSH configuration.";
    };
  };

  config = lib.mkIf cfg.enable {
    users.users.${cfg.user} = {
      isSystemUser = true;
      group = cfg.group;
      description = "org-wiki-web service user";
    };

    users.groups.${cfg.group} = {};

    # Create the socket directory before the socket unit tries to bind.
    systemd.tmpfiles.rules = lib.mkIf (cfg.socket != null) [
      "d ${dirOf cfg.socket} 0750 ${cfg.user} ${cfg.group} -"
    ];

    # Socket unit: systemd creates and holds the Unix domain socket, then
    # passes the open file descriptor to the service on first activation.
    systemd.sockets.org-wiki-web = lib.mkIf (cfg.socket != null) {
      description = "org-wiki-web Unix domain socket";
      wantedBy = ["sockets.target"];
      socketConfig = {
        ListenStream = cfg.socket;
        SocketUser = cfg.user;
        SocketGroup = cfg.group;
        # 0660: accessible to the service user and group only.  Add the
        # reverse proxy user to cfg.group to grant it access.
        SocketMode = "0660";
        Accept = false;
      };
    };

    systemd.services.org-wiki-web = {
      description = "org-wiki-web web service";
      wantedBy = ["multi-user.target"];
      after =
        ["network.target"]
        ++ lib.optional (cfg.socket != null) "org-wiki-web.socket";
      requires =
        lib.optional (cfg.socket != null) "org-wiki-web.socket";

      # git, openssh, and pandoc must be on PATH.  git is used for commits
      # and pushes; openssh provides the ssh binary referenced by
      # GIT_SSH_COMMAND; pandoc renders org-mode to HTML.  org-fmt is
      # linked into the binary, so no extra package is required.
      path = [pkgs.git pkgs.openssh pkgs.pandoc];

      environment =
        {
          LOG_LEVEL = cfg.logLevel;
          LOG_FORMAT = cfg.logFormat;
          LISTEN =
            if cfg.socket != null
            then "sd-listen"
            else "${cfg.host}:${toString cfg.port}";
          FRONTEND_PATH = cfg.frontendPath;
          CONTENT_REPO = cfg.contentRepo;
          PANDOC_BIN = "pandoc";
          SITE_TITLE = cfg.siteTitle;
          COMMIT_AUTHOR_NAME = cfg.commitAuthorName;
          COMMIT_AUTHOR_EMAIL = cfg.commitAuthorEmail;
          OIDC_ISSUER = cfg.oidcIssuer;
          OIDC_CLIENT_ID = cfg.oidcClientId;
          OIDC_CLIENT_SECRET_FILE = "/run/credentials/org-wiki-web.service/oidc-client-secret";
          BASE_URL = cfg.baseUrl;
          FORMAT_ON_SAVE =
            if cfg.formatOnSave
            then "true"
            else "false";
        }
        // lib.optionalAttrs (cfg.contentRemote != null) {
          CONTENT_REMOTE = cfg.contentRemote;
        }
        // lib.optionalAttrs (cfg.cacheDir != null) {
          CACHE_DIR = cfg.cacheDir;
        }
        // lib.optionalAttrs (cfg.webhookSecretFile != null) {
          WEBHOOK_SECRET_FILE = "/run/credentials/org-wiki-web.service/webhook-secret";
        }
        // lib.optionalAttrs (cfg.sshKeyFile != null) {
          GIT_SSH_COMMAND =
            "ssh -i /run/credentials/org-wiki-web.service/ssh-key"
            + " -o StrictHostKeyChecking=accept-new"
            + " -o UserKnownHostsFile=/var/lib/org-wiki-web/.ssh/known_hosts";
        };

      serviceConfig = {
        # Type = notify causes systemd to wait for the binary to call
        # sd_notify(READY=1) before marking the unit active.  The binary
        # does this via the sd-notify crate immediately after the listener
        # is bound.  NotifyAccess = main restricts who may send
        # notifications to the main process only.
        Type = "notify";
        NotifyAccess = "main";

        # Restart if no WATCHDOG=1 heartbeat arrives within 30 s.  The
        # binary reads WATCHDOG_USEC and pings at half this interval (15 s).
        # Override via systemd.services.org-wiki-web.serviceConfig.WatchdogSec.
        WatchdogSec = lib.mkDefault "30s";

        ExecStart = "${cfg.package}/bin/org-wiki-web";

        User = cfg.user;
        Group = cfg.group;
        Restart = "on-failure";
        RestartSec = "5s";

        StateDirectory = cfg.user;

        # ProtectSystem = strict makes the entire filesystem read-only except
        # for paths explicitly granted via StateDirectory, CacheDirectory, or
        # ReadWritePaths.  Grant write access to the cache dir when configured.
        ReadWritePaths = lib.optional (cfg.cacheDir != null) cfg.cacheDir;

        LoadCredential =
          ["oidc-client-secret:${cfg.oidcClientSecretFile}"]
          ++ lib.optional (cfg.webhookSecretFile != null)
          "webhook-secret:${cfg.webhookSecretFile}"
          ++ lib.optional (cfg.sshKeyFile != null)
          "ssh-key:${cfg.sshKeyFile}";

        # Harden the service environment.
        NoNewPrivileges = true;
        PrivateTmp = true;
        ProtectSystem = "strict";
        ProtectHome = true;
      };
    };
  };
}
