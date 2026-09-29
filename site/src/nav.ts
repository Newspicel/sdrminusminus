import { REPOSITORY } from "./seo";

export interface Link {
  label: string;
  href: string;
}

export interface LinkGroup {
  title: string;
  links: Link[];
}

export const APP = "https://app.sdrmm.com";
export const DISCORD = "https://discord.gg/dYaRyGwBNw";
export const DOWNLOAD: Link = { label: "Download", href: "/download.html" };
export const REMOTE: Link = { label: "Remote access", href: "/remote.html" };
export const BUSINESS: Link = { label: "Business", href: "/business.html" };
export const CONTACT = `${BUSINESS.href}#contact`;
export const SIGN_IN: Link = { label: "Sign in", href: APP };

export const PRIMARY: Link[] = [
  { label: "Docs", href: "/introduction.html" },
  { label: "Hardware", href: "/hardware.html" },
  REMOTE,
  BUSINESS,
];

export const COMMUNITY: Link[] = [
  { label: "GitHub", href: REPOSITORY },
  { label: "Discord", href: DISCORD },
];

export const FOOTER: LinkGroup[] = [
  {
    title: "Software",
    links: [
      DOWNLOAD,
      { label: "Docs", href: "/introduction.html" },
      { label: "First receiver", href: "/getting-started/first-receiver.html" },
      { label: "Troubleshooting", href: "/troubleshooting.html" },
      { label: "Build from source", href: "/development/building.html" },
    ],
  },
  {
    title: "Services",
    links: [REMOTE, { label: "Sign in to app.sdrmm.com", href: APP }, BUSINESS],
  },
  { title: "Community", links: COMMUNITY },
];

export const LEGAL: Link[] = [
  { label: "Imprint", href: "/imprint.html" },
  { label: "Privacy", href: "/privacy.html" },
];

function page(path: string): string {
  return path.replace(/\.html$/, "").replace(/\/(index)?$/, "") || "/";
}

export function isCurrent(href: string, pathname: string): boolean {
  return href.startsWith("/") && page(href) === page(pathname);
}
