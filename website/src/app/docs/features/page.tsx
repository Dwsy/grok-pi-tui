"use client";

import { useDict } from "@/i18n/provider";
import Link from "next/link";

type StatusKey =
  | "Native"
  | "Adapted"
  | "Native+Adapted"
  | "Boundary"
  | "Experimental";

const statusColors: Record<StatusKey, string> = {
  Native: "text-grok bg-grok-dim",
  Adapted: "text-accent-bright bg-accent-glow",
  "Native+Adapted": "text-success bg-success/10",
  Boundary: "text-text-tertiary bg-surface",
  Experimental: "text-warning bg-warning/10",
};

export default function FeaturesPage() {
  const dict = useDict();
  const d = dict.docsPages.features;
  return (
    <div>
      <h1 className="text-3xl font-bold tracking-tight">
        {dict.docs.sidebar.features}
      </h1>
      <p className="mt-4 text-lg leading-relaxed text-text-secondary">
        {d.introLead}{" "}
        <strong className="text-text-primary">v0.0.8</strong>. {d.introSSot}{" "}
        <a
          href="https://github.com/Dwsy/grok-pi/blob/main/docs/FEATURE_MATRIX.md"
          className="text-accent hover:underline"
          target="_blank"
          rel="noopener noreferrer"
        >
          FEATURE_MATRIX.md
        </a>
        . {d.statusLabel}{" "}
        <strong className="text-grok">{d.status.Native}</strong> ·{" "}
        <strong className="text-accent-bright">{d.status.Adapted}</strong> ·{" "}
        <strong className="text-success">{d.status["Native+Adapted"]}</strong>{" "}
        · <strong className="text-text-tertiary">{d.status.Boundary}</strong> ·{" "}
        <strong className="text-warning">{d.status.Experimental}</strong>.
      </p>
      <p className="mt-3 text-sm text-text-secondary">
        {d.deepDives}{" "}
        <Link href="/docs/extensions/" className="text-accent hover:underline">
          {dict.docs.sidebar.extensions}
        </Link>
        {" · "}
        <Link href="/docs/commands/" className="text-accent hover:underline">
          {dict.docs.sidebar.commands}
        </Link>
        {" · "}
        <Link
          href="/docs/configuration/"
          className="text-accent hover:underline"
        >
          {dict.docs.sidebar.configuration}
        </Link>
      </p>

      {d.sections.map((section) => (
        <div key={section.title} className="mt-10">
          <h2 className="text-xl font-semibold mb-4">{section.title}</h2>
          <div className="overflow-x-auto rounded-md border border-border">
            <table className="w-full text-sm">
              <thead>
                <tr className="border-b border-border bg-surface/80">
                  <th className="px-4 py-3 text-left font-semibold text-text-secondary">
                    {d.cols.feature}
                  </th>
                  <th className="px-4 py-3 text-left font-semibold text-text-secondary">
                    {d.cols.status}
                  </th>
                  <th className="px-4 py-3 text-left font-semibold text-text-secondary">
                    {d.cols.notes}
                  </th>
                </tr>
              </thead>
              <tbody>
                {section.rows.map((row, i) => {
                  const status = row.status as StatusKey;
                  return (
                    <tr
                      key={row.feature}
                      className={`border-b border-border/50 ${i % 2 === 1 ? "bg-surface/20" : ""}`}
                    >
                      <td className="px-4 py-3 font-medium text-text-primary whitespace-nowrap">
                        {row.feature}
                      </td>
                      <td className="px-4 py-3">
                        <span
                          className={`inline-block px-2 py-0.5 rounded-sm text-xs font-medium ${statusColors[status]}`}
                        >
                          {d.status[status]}
                        </span>
                      </td>
                      <td className="px-4 py-3 text-text-secondary text-xs">
                        {row.notes}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        </div>
      ))}
    </div>
  );
}
