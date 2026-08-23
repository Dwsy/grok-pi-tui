"use client";

import { useDict } from "@/i18n/provider";
import Link from "next/link";

const layerStyles = [
  { color: "border-grok/30", bg: "bg-grok-dim" },
  { color: "border-accent/30", bg: "bg-accent-glow" },
  { color: "border-success/30", bg: "bg-success/10" },
] as const;

export default function ArchitecturePage() {
  const dict = useDict();
  const d = dict.docsPages.architecture;
  return (
    <div>
      <h1 className="text-3xl font-bold tracking-tight">
        {dict.docs.sidebar.architecture}
      </h1>
      <p className="mt-4 text-lg leading-relaxed text-text-secondary">
        {d.intro}
      </p>

      <h2 className="mt-10 text-xl font-semibold">{d.layersTitle}</h2>
      <div className="mt-6 space-y-4">
        {d.layers.map((layer, i) => {
          const style = layerStyles[i] ?? layerStyles[0];
          return (
            <div
              key={layer.name}
              className={`rounded-md border ${style.color} ${style.bg} p-5`}
            >
              <h3 className="font-mono text-sm font-semibold text-text-primary">
                {layer.name}
              </h3>
              <p className="mt-1 text-xs text-text-tertiary">{layer.role}</p>
              <ul className="mt-4 space-y-1.5">
                {layer.details.map((detail) => (
                  <li
                    key={detail}
                    className="flex items-start gap-2 text-sm text-text-secondary"
                  >
                    <span className="mt-1 w-1 h-1 rounded-full bg-text-tertiary shrink-0" />
                    {detail}
                  </li>
                ))}
              </ul>
            </div>
          );
        })}
      </div>

      <h2 className="mt-10 text-xl font-semibold">{d.runtimeTitle}</h2>
      <div className="mt-4 grid gap-3 sm:grid-cols-2">
        {d.runtime.map((r) => (
          <div
            key={r.title}
            className="rounded-md border border-border bg-surface p-4"
          >
            <h3 className="text-sm font-medium text-text-primary">{r.title}</h3>
            <p className="mt-1.5 text-xs leading-relaxed text-text-secondary">
              {r.body}
            </p>
          </div>
        ))}
      </div>
      <p className="mt-4 text-sm text-text-secondary">
        {d.fieldMap}{" "}
        <Link href="/docs/features/" className="text-accent hover:underline">
          {d.featureMatrix}
        </Link>
        {" · "}
        <Link href="/docs/extensions/" className="text-accent hover:underline">
          {d.extensions}
        </Link>
        {" · "}
        <a
          href="https://github.com/Dwsy/grok-pi/blob/main/docs/NATIVE_GROK_TUI_ALIGNMENT.md"
          className="text-accent hover:underline"
          target="_blank"
          rel="noopener noreferrer"
        >
          NATIVE_GROK_TUI_ALIGNMENT.md
        </a>
      </p>

      <h2 className="mt-10 text-xl font-semibold">{d.seamsTitle}</h2>
      <p className="mt-2 text-text-secondary">{d.seamsLead}</p>
      <div className="mt-4 space-y-2">
        {d.seams.map((seam) => (
          <div
            key={seam.file}
            className="flex flex-col sm:flex-row sm:items-start gap-1 sm:gap-3 rounded-md border border-border bg-surface px-4 py-3"
          >
            <code className="font-mono text-xs text-accent whitespace-nowrap sm:mt-0.5">
              {seam.file}
            </code>
            <span className="text-sm text-text-secondary">{seam.desc}</span>
          </div>
        ))}
      </div>

      <h2 className="mt-10 text-xl font-semibold">{d.invariantsTitle}</h2>
      <ul className="mt-3 space-y-1.5 text-sm text-text-secondary">
        {d.invariants.map((inv) => (
          <li key={inv} className="flex items-center gap-2">
            <span className="text-success">✓</span> {inv}
          </li>
        ))}
      </ul>
    </div>
  );
}
