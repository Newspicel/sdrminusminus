export interface LegalEntry {
  title: string;
  body: string;
}

const operator = "Julian Haag – Informatiker\nLieselsweg 11\n53945 Blankenheim\nGermany";

export const imprint: LegalEntry[] = [
  {
    title: "Information pursuant to § 5 DDG",
    body: "Julian Haag – Informatiker\nSole proprietor: Julian Haag\nLieselsweg 11\n53945 Blankenheim\nGermany",
  },
  {
    title: "Contact",
    body: "Email: me@jhaag.me",
  },
  {
    title: "VAT identification number",
    body: "VAT ID pursuant to § 27a UStG: DE363625397",
  },
  {
    title: "Professional title",
    body: "Freelance computer scientist",
  },
  {
    title: "Responsible for content according to § 18 (2) MStV",
    body: "Julian Haag (address as above).",
  },
  {
    title: "Dispute resolution",
    body: "I am neither willing nor obliged to participate in dispute settlement proceedings before a consumer arbitration board.",
  },
  {
    title: "Liability for content",
    body: "The content of these pages has been created with the greatest care. I accept no liability for the accuracy, completeness or timeliness of the content. As a service provider I am responsible for my own content according to § 7 (1) DDG. Pursuant to §§ 8 to 10 DDG, however, I am not obliged to monitor transmitted or stored third-party information.",
  },
  {
    title: "Liability for links",
    body: "This site contains links to external third-party websites over whose content I have no influence. I cannot accept any liability for this external content. The respective provider or operator of the linked pages is always responsible for their content.",
  },
];

export const privacy: LegalEntry[] = [
  {
    title: "In short",
    body: "This site processes as little data as possible. No tracking, no analytics, no cookies, no contact form. The sections below explain what still gets processed, and why.",
  },
  {
    title: "Controller",
    body: `${operator}\nEmail: me@jhaag.me`,
  },
  {
    title: "Hosting and server logs",
    body: "This site is hosted on GitHub Pages by GitHub, Inc., 88 Colin P. Kelly Jr. Street, San Francisco, CA 94107, USA. GitHub records the IP address of every visitor to keep the service secure and running. Legal basis: Art. 6 (1) (f) GDPR (legitimate interest in delivering the site reliably and securely). GitHub is certified under the EU-U.S. Data Privacy Framework, which covers the transfer to the USA (Art. 45 GDPR).",
  },
  {
    title: "Download page",
    body: "To show the latest release and its file sizes, your browser asks the GitHub API (api.github.com). Downloads come straight from GitHub. GitHub receives your IP address and user agent. Legal basis: Art. 6 (1) (f) GDPR (legitimate interest in offering the right file).",
  },
  {
    title: "Live demo map",
    body: "The Aircraft and Ships demo scenes show a map. Its style, tiles and fonts load from OpenFreeMap (tiles.openfreemap.org), which receives your IP address and user agent. Legal basis: Art. 6 (1) (f) GDPR (legitimate interest in showing the demo).",
  },
  {
    title: "Local storage",
    body: "The docs remember theme and sidebar (mdbook-theme, mdbook-sidebar). The demo remembers display choices under keys starting with sdrmm. These values stay in your browser, are never sent to a server and contain no personal data. They are strictly necessary for functions you use (§ 25 (2) no. 2 TDDDG). You can clear them in your browser settings at any time.",
  },
  {
    title: "Fonts and links",
    body: "Fonts are served from this site. No connection is made to Google Fonts or similar services. Links to GitHub, Discord, YouTube, Reddit and X are plain links: nothing loads from these services until you follow one.",
  },
  {
    title: "Contact by email",
    body: "When you contact me by email, the data you send (email address, name, message content) is processed solely to handle your request. Legal basis: Art. 6 (1) (b) GDPR for contract-related requests, otherwise Art. 6 (1) (f) GDPR. The data is deleted once it is no longer needed and no statutory retention obligations apply.",
  },
  {
    title: "Your rights",
    body: "You have the right of access (Art. 15 GDPR), rectification (Art. 16), erasure (Art. 17), restriction of processing (Art. 18), data portability (Art. 20) and objection (Art. 21). An informal email to me@jhaag.me is sufficient to exercise these rights.",
  },
  {
    title: "Right to lodge a complaint",
    body: "You have the right to lodge a complaint with a data protection supervisory authority. The competent authority is the Landesbeauftragte für Datenschutz und Informationsfreiheit Nordrhein-Westfalen (LDI NRW), Kavalleriestr. 2–4, 40213 Düsseldorf, Germany.",
  },
];
