{
  lib,
  rustPlatform,
  fetchPnpmDeps,
  pnpmConfigHook,
  pnpm,
  nodejs_26,
  pkg-config,
  cmake,
  copyDesktopItems,
  makeDesktopItem,
  wrapGAppsHook3,
  cairo,
  gdk-pixbuf,
  glib,
  gtk3,
  libayatana-appindicator,
  libopus,
  librsvg,
  libsoup_3,
  openssl,
  pango,
  soapysdr,
  webkitgtk_4_1,
  xdotool,
  soapyPlugins ? [ ],
}:

rustPlatform.buildRustPackage (finalAttrs: {
  pname = "sdrmm-desktop";
  version = (builtins.fromTOML (builtins.readFile ../../Cargo.toml)).workspace.package.version;

  src = lib.cleanSource ../..;

  cargoLock = {
    lockFile = ../../Cargo.lock;
    outputHashes = {
      "soapysdr-0.5.1" = "sha256-Elx3hBXeQAzeJjMOZ5QJ3d5gjOoSLhR2NoL40rwu3U8=";
    };
  };

  pnpmDeps = fetchPnpmDeps {
    inherit (finalAttrs) pname version src;
    inherit pnpm;
    sourceRoot = "${finalAttrs.src.name}/web";
    fetcherVersion = 4;
    hash = "sha256-0EISTOes7WnkZQ2p249D0p+KA6mxkynLehPO6v3kqQ0=";
  };
  pnpmRoot = "web";

  nativeBuildInputs = [
    cmake
    copyDesktopItems
    nodejs_26
    pkg-config
    pnpm
    pnpmConfigHook
    wrapGAppsHook3
  ];

  buildInputs = [
    cairo
    gdk-pixbuf
    glib
    gtk3
    libayatana-appindicator
    libopus
    librsvg
    libsoup_3
    openssl
    pango
    soapysdr
    webkitgtk_4_1
    xdotool
  ];

  preBuild = ''
    pnpm --dir web build
  '';

  cargoBuildFlags = [
    "--package"
    "sdrmm-desktop"
    "--no-default-features"
    "--features"
    "soapy,net-client"
  ];
  cargoTestFlags = finalAttrs.cargoBuildFlags;

  desktopItems = [
    (makeDesktopItem {
      name = "sdrmm-desktop";
      desktopName = "sdr--";
      comment = "Software-defined radio receiver";
      exec = "sdrmm-desktop";
      icon = "dev.newspicel.sdrmm";
      categories = [
        "AudioVideo"
        "HamRadio"
      ];
    })
  ];

  postInstall = ''
    install -Dm644 apps/desktop/icons/128x128.png \
      "$out/share/icons/hicolor/128x128/apps/dev.newspicel.sdrmm.png"
    install -Dm644 apps/desktop/icons/128x128@2x.png \
      "$out/share/icons/hicolor/256x256/apps/dev.newspicel.sdrmm.png"
  '';

  preFixup = lib.optionalString (soapyPlugins != [ ]) ''
    gappsWrapperArgs+=(
      --prefix SOAPY_SDR_PLUGIN_PATH : "${lib.makeSearchPath soapysdr.searchPath soapyPlugins}"
    )
  '';

  passthru = {
    inherit soapyPlugins;
  };

  meta = {
    description = "Modular software-defined radio receiver desktop application";
    homepage = "https://github.com/Newspicel/sdrminusminus";
    license = lib.licenses.gpl3Plus;
    mainProgram = "sdrmm-desktop";
    platforms = lib.platforms.linux;
  };
})
