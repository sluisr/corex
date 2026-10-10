import React, { useState } from 'react';
import { Terminal, Copy, Check, Smartphone, Monitor, Apple, Download } from 'lucide-react';

export const InstallSection: React.FC = () => {
  const [copiedIndex, setCopiedIndex] = useState<number | null>(null);

  const methods = [
    {
      os: 'Linux & macOS',
      icon: Apple,
      desc: 'Automatic architecture detection (Intel / Apple Silicon / x86_64) with SHA-256 verification.',
      cmd: 'curl -fsSL https://raw.githubusercontent.com/sluisr/corex/main/install.sh | sh',
    },
    {
      os: 'Android (Termux)',
      icon: Smartphone,
      desc: 'Native aarch64-linux-android binary. Installs directly to $PREFIX/bin without root.',
      cmd: 'curl -fsSL https://raw.githubusercontent.com/sluisr/corex/main/install.sh | sh',
    },
    {
      os: 'NPM (Universal / Windows)',
      icon: Monitor,
      desc: 'Multiplatform bootstrap installer with dynamic SHA-256 integrity checks.',
      cmd: 'npm install -g @sluisr/corex',
    },
    {
      os: 'From Source (Cargo)',
      icon: Terminal,
      desc: 'Compile with native machine optimizations directly from the latest git commit.',
      cmd: 'cargo install --git https://github.com/sluisr/corex.git --force',
    },
  ];

  const handleCopy = (cmd: string, idx: number) => {
    navigator.clipboard.writeText(cmd);
    setCopiedIndex(idx);
    setTimeout(() => setCopiedIndex(null), 2000);
  };

  return (
    <section id="install" className="py-16 md:py-24 max-w-5xl mx-auto px-4 sm:px-6 lg:px-8">
      <div className="text-center max-w-3xl mx-auto mb-14">
        <div className="inline-flex items-center gap-2 px-3 py-1 rounded-full bg-surface-100 border border-border-subtle text-xs font-mono text-cyan-300 mb-4">
          <Download className="w-3.5 h-3.5 text-cyan-400" />
          <span>Quickstart</span>
        </div>
        <h2 className="text-3xl sm:text-4xl font-bold tracking-tight text-white mb-4 font-sans">
          Ready in Under 10 Seconds
        </h2>
        <p className="text-gray-400 text-base sm:text-lg font-normal">
          Zero complex dependencies. Single binary installation across all platforms.
        </p>
      </div>

      <div className="grid grid-cols-1 md:grid-cols-2 gap-5">
        {methods.map((m, idx) => {
          const Icon = m.icon;
          const isCopied = copiedIndex === idx;

          return (
            <div
              key={idx}
              className="p-5 rounded-xl bg-surface-100/80 border border-border-subtle hover:border-border-hover transition-all glow-card flex flex-col justify-between"
            >
              <div>
                <div className="flex items-center gap-2.5 mb-2">
                  <div className="w-8 h-8 rounded-lg bg-surface-200 border border-border-subtle flex items-center justify-center text-cyan-400">
                    <Icon className="w-4 h-4" />
                  </div>
                  <h3 className="font-bold text-white text-base font-sans">{m.os}</h3>
                </div>
                <p className="text-xs text-gray-400 mb-4">{m.desc}</p>
              </div>

              <div className="relative flex items-center justify-between p-2.5 rounded-lg bg-surface-200 border border-border-subtle font-mono text-xs text-gray-200 group">
                <code className="truncate mr-2 text-cyan-300">{m.cmd}</code>
                <button
                  onClick={() => handleCopy(m.cmd, idx)}
                  className="px-2 py-1 rounded bg-surface-100 hover:bg-surface-50 border border-border-subtle text-xs text-gray-300 hover:text-white transition-all flex items-center gap-1 flex-shrink-0"
                >
                  {isCopied ? (
                    <Check className="w-3.5 h-3.5 text-emerald-400" />
                  ) : (
                    <Copy className="w-3.5 h-3.5 text-gray-400" />
                  )}
                </button>
              </div>
            </div>
          );
        })}
      </div>
    </section>
  );
};
