"use client";

import { useDict } from "@/i18n/provider";

export function Footer() {
  const dict = useDict();
  const { footer } = dict;

  const links = [
    { label: footer.links.github, href: "https://github.com/Dwsy/grok-pi" },
    { label: footer.links.changelog, href: "https://github.com/Dwsy/grok-pi/blob/main/CHANGELOG.MD" },
    { label: footer.links.featureMatrix, href: "https://github.com/Dwsy/grok-pi/blob/main/docs/FEATURE_MATRIX.md" },
    { label: footer.links.license, href: "https://github.com/Dwsy/grok-pi/blob/main/LICENSE" },
  ];

  return (
    <footer className="border-t border-border py-12 px-6">
      <div className="mx-auto max-w-6xl flex flex-col sm:flex-row items-center justify-between gap-6">
        <div className="flex items-center gap-2">
          <span className="font-mono text-lg font-bold text-accent">π</span>
          <span className="text-sm text-text-tertiary">{footer.tagline}</span>
        </div>
        <nav className="flex items-center gap-6">
          {links.map((l) => (
            <a
              key={l.label}
              href={l.href}
              target="_blank"
              rel="noopener noreferrer"
              className="text-xs text-text-tertiary hover:text-text-secondary transition-colors"
            >
              {l.label}
            </a>
          ))}
        </nav>
      </div>
      <p className="mt-8 text-center text-[11px] text-text-tertiary/60">{footer.copyright}</p>
    </footer>
  );
}


export default Footer;
