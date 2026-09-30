export interface Install {
  id: string;
  tab: string;
  title: string;
  note: string;
  lines: string;
  more?: { href: string; label: string };
}

export const INSTALLS: Install[] = [
  {
    id: "brew",
    tab: "Homebrew",
    title: "Homebrew",
    note: "The app on macOS. The server on macOS or Linux.",
    lines: `brew install newspicel/tap/sdrmm-app

<span class="p">or the server</span>
brew install newspicel/tap/sdrmm
brew services start sdrmm`,
  },
  {
    id: "winget",
    tab: "WinGet",
    title: "WinGet",
    note: "The Windows desktop app.",
    lines: "winget install Newspicel.SDRminusminus",
  },
  {
    id: "apt",
    tab: "APT",
    title: "Debian and Ubuntu",
    note: "Signed APT repository for x86-64 and ARM64.",
    lines: `curl -fsSL https://newspicel.github.io/packages/key.gpg \\
  | sudo tee /usr/share/keyrings/sdrminusminus.gpg > /dev/null
echo "deb [signed-by=/usr/share/keyrings/sdrminusminus.gpg] \\
https://newspicel.github.io/packages/deb stable main" \\
  | sudo tee /etc/apt/sources.list.d/sdrminusminus.list
sudo apt update
sudo apt install sdrminusminus`,
  },
  {
    id: "dnf",
    tab: "DNF",
    title: "Fedora",
    note: "Signed RPM repository for x86-64 and ARM64.",
    lines: `sudo dnf config-manager addrepo \\
  --from-repofile=https://newspicel.github.io/packages/rpm/sdrminusminus.repo
sudo dnf install sdrminusminus`,
  },
  {
    id: "nix",
    tab: "Nix",
    title: "Nix",
    note: "Flake package for x86-64 and ARM64 Linux, with SoapySDR modules selectable.",
    lines: `nix --extra-experimental-features 'nix-command flakes' \\
  profile install github:Newspicel/sdrminusminus
sdrmm-desktop`,
    more: { href: "/docs/getting-started/install#nix", label: "NixOS module options" },
  },
  {
    id: "container",
    tab: "Container",
    title: "Container",
    note: "Keeps its database and recordings in a volume. Pass through the USB group that owns your radio.",
    lines: `git clone https://github.com/Newspicel/sdrminusminus.git
cd sdrminusminus
docker compose up -d`,
    more: { href: "/docs/server/deployment", label: "Container setup, auth and HTTPS" },
  },
];

export function installFor(agent: string): string {
  if (/Android|iPhone|iPad/.test(agent)) {
    return "brew";
  }
  if (/Windows/.test(agent)) {
    return "winget";
  }
  if (/Fedora|Red Hat|CentOS|Rocky|AlmaLinux|SUSE/i.test(agent)) {
    return "dnf";
  }
  if (/Linux|X11|CrOS/.test(agent)) {
    return "apt";
  }
  return "brew";
}
