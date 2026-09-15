{
  lib,
  stdenv,
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
      # git rev 6a768a2f843099171d7ed08df9fe0f3ba0678f25
      "xng-acars-0.21.0" = "sha256-Gaws7KiS6VDkJdctJV9vzvFfWEInDGf7GledbLmouUk=";
    };
  };

  pnpmDeps = fetchPnpmDeps {
    inherit (finalAttrs) pname version src;
    inherit pnpm;
    sourceRoot = "${finalAttrs.src.name}/web";
    fetcherVersion = 4;
    # web/pnpm-lock.yaml sha256:7a7df674fa297a4217cacd330fcc6ac35d3e953ae7e993ea91df9df79555a32e
    hash = "sha256-4Bg6DVlnWrJkxCvZIqqBaswy3WaMl6jpkUHf0xL7UV0=";
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
      desktopName = "SDR--";
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

  # Nothing links SoapySDR: it is opened at runtime, and outside a Nix store there is no
  # default path to find it on. The wrapper names the store copy, and the plugins the user
  # selected stay separate packages it merely points at.
  preFixup = ''
    gappsWrapperArgs+=(
      --set-default SDRMM_SOAPY_LIBRARY "${soapysdr}/lib/libSoapySDR${stdenv.hostPlatform.extensions.sharedLibrary}"
    )
  '' + lib.optionalString (soapyPlugins != [ ]) ''
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
