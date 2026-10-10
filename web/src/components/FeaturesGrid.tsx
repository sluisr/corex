import React from 'react';
import { Cpu, Globe, Smartphone, ShieldCheck, FileCode, Lock, Zap, HardDrive } from 'lucide-react';

export const FeaturesGrid: React.FC = () => {
  const features = [
    {
      icon: Cpu,
      title: 'Native Rust Engine',
      description: 'Zero runtime overhead. Starts in less than 5ms, operates with minimal RAM footprint, and automatically reclaims free memory pages back to the OS via glibc malloc_trim.',
      tag: '< 5ms Boot',
      tagColor: 'text-cyan-400 border-cyan-500/30 bg-cyan-500/10',
    },
    {
      icon: Globe,
      title: 'Native Official Protocols',
      description: 'Direct wire protocol support without lossy middleman translation. Speaks Anthropic /v1/messages SSE, Google Gemini thought signatures, OpenAI reasoning_effort, and local LLMs.',
      tag: 'Multi-Protocol',
      tagColor: 'text-violet-400 border-violet-500/30 bg-violet-500/10',
    },
    {
      icon: Smartphone,
      title: 'Android & Termux Optimized',
      description: 'First CLI agent built for mobile. Automatically detects portrait viewports (< 68 cols), disables terminal mouse reporting so screen taps open the software keyboard instantly.',
      tag: 'Termux Ready',
      tagColor: 'text-emerald-400 border-emerald-500/30 bg-emerald-500/10',
    },
    {
      icon: ShieldCheck,
      title: 'Hardened YOLO Sandbox',
      description: 'Run fully autonomous without constant prompts. Features protected path guards (SSH keys, GPG credentials, system files) and configurable allowed_commands whitelisting.',
      tag: 'YOLO Mode',
      tagColor: 'text-amber-400 border-amber-500/30 bg-amber-500/10',
    },
    {
      icon: FileCode,
      title: 'Fuzzy Patch Engine',
      description: 'High-tolerance diff application engine that gracefully handles LLM whitespace variations, leading/trailing markdown fences, and strict CRLF versus LF line ending preservation.',
      tag: 'Zero File Corruption',
      tagColor: 'text-sky-400 border-sky-500/30 bg-sky-500/10',
    },
    {
      icon: Lock,
      title: '100% Private & Air-Gapped',
      description: 'Use local models via Ollama or llama.cpp with $0 API cost and zero telemetry. Settings and API keys remain encrypted on your disk, never sent to third-party tracking servers.',
      tag: 'Offline Capable',
      tagColor: 'text-rose-400 border-rose-500/30 bg-rose-500/10',
    },
  ];

  return (
    <section id="features" className="py-16 md:py-24 max-w-7xl mx-auto px-4 sm:px-6 lg:px-8">
      <div className="text-center max-w-3xl mx-auto mb-16">
        <div className="inline-flex items-center gap-2 px-3 py-1 rounded-full bg-surface-100 border border-border-subtle text-xs font-mono text-cyan-300 mb-4">
          <Zap className="w-3.5 h-3.5 text-cyan-400" />
          <span>Architectural Superiority</span>
        </div>
        <h2 className="text-3xl sm:text-4xl font-bold tracking-tight text-white mb-4 font-sans">
          Engineered from Scratch for Serious Developers
        </h2>
        <p className="text-gray-400 text-base sm:text-lg font-normal">
          No sluggish Python wrappers or electron bloat. Corex is designed for pure terminal efficiency and autonomous safety.
        </p>
      </div>

      <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-6">
        {features.map((feature, idx) => {
          const Icon = feature.icon;
          return (
            <div
              key={idx}
              className="p-6 rounded-2xl bg-surface-100/70 border border-border-subtle hover:border-border-hover transition-all group glow-card flex flex-col justify-between"
            >
              <div>
                <div className="flex items-center justify-between mb-4">
                  <div className="w-10 h-10 rounded-xl bg-surface-200 border border-border-subtle flex items-center justify-center text-cyan-400 group-hover:scale-105 group-hover:border-cyan-500/30 transition-all">
                    <Icon className="w-5 h-5" />
                  </div>
                  <span className={`text-[11px] font-mono font-medium px-2.5 py-0.5 rounded-full border ${feature.tagColor}`}>
                    {feature.tag}
                  </span>
                </div>

                <h3 className="text-lg font-semibold text-white mb-2 font-sans">
                  {feature.title}
                </h3>
                <p className="text-sm text-gray-400 leading-relaxed">
                  {feature.description}
                </p>
              </div>

              <div className="mt-6 pt-4 border-t border-border-subtle/50 flex items-center gap-2 text-xs font-mono text-gray-400">
                <HardDrive className="w-3.5 h-3.5" />
                <span>Rust 2021 Edition</span>
              </div>
            </div>
          );
        })}
      </div>
    </section>
  );
};
