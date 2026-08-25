"use client";

import { useDict } from "@/i18n/provider";
import CodeBlock from "@/components/ui/CodeBlock";
import Link from "next/link";

export default function InstallationPage() {
  const dict = useDict();
  const d = dict.docsPages.installation;
  return (
    <div>
      <h1 className="text-3xl font-bold tracking-tight">
        {dict.docs.sidebar.installation}
      </h1>
      <p className="mt-4 text-lg leading-relaxed text-text-secondary">
        {d.introLead}{" "}
        <strong className="text-text-primary">0.84.3</strong>
        {d.introMid}{" "}
        <strong className="text-text-primary">0.0.8</strong>
        {d.introEnd}
      </p>

      <h2 className="mt-10 text-xl font-semibold">{d.sections.macos}</h2>
      <p className="mt-2 text-text-secondary">{d.macosBody}</p>
      <div className="mt-4">
        <CodeBlock
          code="curl -fsSL https://github.com/Dwsy/grok-pi/releases/latest/download/install.sh | sh"
          label={d.installLabel}
        />
      </div>
      <p className="mt-3 text-sm text-text-tertiary">
        {d.overrideInstall}{" "}
        <code className="font-mono text-accent">GROK_PI_INSTALL_DIR</code>.
      </p>

      <h2 className="mt-10 text-xl font-semibold">{d.sections.windows}</h2>
      <div className="mt-4">
        <CodeBlock
          code="irm https://github.com/Dwsy/grok-pi/releases/latest/download/install.ps1 | iex"
          label={d.powershellLabel}
        />
      </div>

      <h2 className="mt-10 text-xl font-semibold">{d.sections.prereq}</h2>
      <div className="mt-4">
        <CodeBlock
          code="npm install --global @earendil-works/pi-coding-agent"
          label={d.installPiLabel}
        />
      </div>
      <p className="mt-3 text-sm text-text-secondary">{d.prereqBody}</p>

      <h2 className="mt-10 text-xl font-semibold">{d.sections.firstRun}</h2>
      <div className="mt-4 space-y-3">
        <CodeBlock code="cd your-project && grok-pi" label={d.newSession} />
        <CodeBlock code="grok-pi --continue" label={d.continueSession} />
        <CodeBlock
          code="grok-pi --session 019f88c"
          label={d.resumePartial}
        />
      </div>
      <p className="mt-3 text-sm text-text-secondary">
        {d.migrationHint}{" "}
        <Link href="/docs/migration/" className="text-accent hover:underline">
          {d.migrationLink}
        </Link>
        .
      </p>

      <h2 className="mt-10 text-xl font-semibold">{d.sections.verify}</h2>
      <div className="mt-4">
        <CodeBlock code="grok-pi --version" label={d.checkVersion} />
      </div>
      <p className="mt-3 text-sm text-text-tertiary">{d.expectVersion}</p>

      <h2 className="mt-10 text-xl font-semibold">{d.sections.updating}</h2>
      <div className="mt-4 space-y-3">
        <CodeBlock code="grok-pi update --check" label={d.updateCheck} />
        <CodeBlock code="grok-pi update" label={d.updateInstall} />
      </div>
      <p className="mt-3 text-sm text-text-tertiary">{d.updateNote}</p>

      <h2 className="mt-10 text-xl font-semibold">{d.sections.crash}</h2>
      <p className="mt-2 text-text-secondary">{d.crashBody}</p>
      <div className="mt-4">
        <CodeBlock code="grok-pi -ne" label={d.noExtLabel} />
      </div>

      <h2 className="mt-10 text-xl font-semibold">{d.sections.source}</h2>
      <p className="mt-2 text-text-secondary">{d.sourceBody}</p>
      <div className="mt-4 space-y-3">
        <CodeBlock code="./build.sh" label={d.buildLabel} />
        <CodeBlock
          code="PI_BIN=pi ./run-local.sh /path/to/project"
          label={d.runLocalLabel}
        />
      </div>
    </div>
  );
}
