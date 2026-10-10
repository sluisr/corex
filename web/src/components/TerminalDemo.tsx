import React, { useState } from 'react';
import { Terminal as TerminalIcon, Sparkles, Smartphone, Layers, Play } from 'lucide-react';
import { motion, AnimatePresence } from 'framer-motion';

type DemoScenario = 'yolo' | 'gemini' | 'termux' | 'models';

export const TerminalDemo: React.FC = () => {
  const [activeScenario, setActiveScenario] = useState<DemoScenario>('yolo');

  return (
    <section id="demo" className="py-12 md:py-20 max-w-6xl mx-auto px-4 sm:px-6 lg:px-8">
      <div className="text-center mb-10">
        <h2 className="text-2xl sm:text-3xl font-bold tracking-tight text-white mb-3 font-sans">
          See Corex in Action
        </h2>
        <p className="text-gray-400 text-sm sm:text-base max-w-2xl mx-auto font-normal">
          Autonomous tool execution, thought signature streaming, and mobile responsive layout in a native Rust TUI.
        </p>
      </div>

      {/* Scenario Switcher Tabs */}
      <div className="flex flex-wrap items-center justify-center gap-2 mb-6">
        <button
          onClick={() => setActiveScenario('yolo')}
          className={`flex items-center gap-2 px-3.5 py-2 rounded-lg text-xs font-mono font-medium transition-all ${
            activeScenario === 'yolo'
              ? 'bg-cyan-500/10 text-cyan-300 border border-cyan-500/30 shadow-sm'
              : 'bg-surface-100 text-gray-400 hover:text-gray-200 border border-border-subtle'
          }`}
        >
          <Play className="w-3.5 h-3.5" />
          <span>Autonomous YOLO Task</span>
        </button>

        <button
          onClick={() => setActiveScenario('gemini')}
          className={`flex items-center gap-2 px-3.5 py-2 rounded-lg text-xs font-mono font-medium transition-all ${
            activeScenario === 'gemini'
              ? 'bg-cyan-500/10 text-cyan-300 border border-cyan-500/30 shadow-sm'
              : 'bg-surface-100 text-gray-400 hover:text-gray-200 border border-border-subtle'
          }`}
        >
          <Sparkles className="w-3.5 h-3.5" />
          <span>Gemini 3.x Thought Signatures</span>
        </button>

        <button
          onClick={() => setActiveScenario('termux')}
          className={`flex items-center gap-2 px-3.5 py-2 rounded-lg text-xs font-mono font-medium transition-all ${
            activeScenario === 'termux'
              ? 'bg-cyan-500/10 text-cyan-300 border border-cyan-500/30 shadow-sm'
              : 'bg-surface-100 text-gray-400 hover:text-gray-200 border border-border-subtle'
          }`}
        >
          <Smartphone className="w-3.5 h-3.5" />
          <span>Termux Mobile Portrait</span>
        </button>

        <button
          onClick={() => setActiveScenario('models')}
          className={`flex items-center gap-2 px-3.5 py-2 rounded-lg text-xs font-mono font-medium transition-all ${
            activeScenario === 'models'
              ? 'bg-cyan-500/10 text-cyan-300 border border-cyan-500/30 shadow-sm'
              : 'bg-surface-100 text-gray-400 hover:text-gray-200 border border-border-subtle'
          }`}
        >
          <Layers className="w-3.5 h-3.5" />
          <span>Multi-Provider Switcher (/model)</span>
        </button>
      </div>

      {/* Terminal Window Box */}
      <div className="rounded-2xl border border-border-subtle bg-surface-300 shadow-2xl overflow-hidden font-mono text-xs sm:text-sm glow-card">
        {/* Terminal Header */}
        <div className="h-10 px-4 bg-surface-200/90 border-b border-border-subtle flex items-center justify-between">
          <div className="flex items-center gap-2">
            <div className="w-3 h-3 rounded-full bg-red-500/80" />
            <div className="w-3 h-3 rounded-full bg-yellow-500/80" />
            <div className="w-3 h-3 rounded-full bg-emerald-500/80" />
            <span className="text-gray-400 text-xs ml-2 flex items-center gap-1.5 font-medium">
              <TerminalIcon className="w-3.5 h-3.5 text-cyan-400" />
              <span>cx — corex-tui (Rust Engine)</span>
            </span>
          </div>

          <div className="flex items-center gap-3 text-xs text-gray-400">
            <span className="hidden sm:inline-flex items-center gap-1 text-emerald-400 bg-emerald-500/10 px-2 py-0.5 rounded border border-emerald-500/20">
              ● YOLO MODE ON
            </span>
            <span className="text-gray-400">80x24</span>
          </div>
        </div>

        {/* Terminal Screen Content */}
        <div className="p-4 sm:p-6 min-h-[380px] bg-background/95 text-gray-200 leading-relaxed overflow-x-auto">
          <AnimatePresence mode="wait">
            {activeScenario === 'yolo' && (
              <motion.div
                key="yolo"
                initial={{ opacity: 0, y: 6 }}
                animate={{ opacity: 1, y: 0 }}
                exit={{ opacity: 0, y: -6 }}
                transition={{ duration: 0.2 }}
                className="space-y-4"
              >
                {/* User Prompt */}
                <div className="flex items-start gap-2">
                  <span className="text-cyan-400 font-bold select-none">&gt;</span>
                  <div>
                    <span className="text-gray-400">You: </span>
                    <span className="text-white font-semibold">create system info script and run it</span>
                  </div>
                </div>

                {/* Assistant Thought & Action */}
                <div className="border-l-2 border-cyan-500/40 pl-3 py-1 space-y-2 text-gray-300">
                  <div className="text-xs text-cyan-300 font-medium flex items-center gap-1.5">
                    <span className="animate-pulse">⣾</span> Corex [Gemini 3.5 Flash · YOLO Auto-Approved]
                  </div>
                  <p className="text-gray-400 text-xs">
                    I'll create a lightweight script to gather CPU, RAM, OS, and storage details, make it executable, and execute it.
                  </p>
                </div>

                {/* Tool 1: write_file */}
                <div className="bg-surface-100 p-3 rounded-lg border border-border-subtle space-y-1.5">
                  <div className="flex items-center justify-between text-xs text-gray-400">
                    <span className="text-emerald-400 font-semibold">⚡ [tool_call] write_file</span>
                    <span className="text-gray-400">Target: ./sysinfo.sh (auto-approved)</span>
                  </div>
                  <pre className="text-gray-300 text-xs overflow-x-auto bg-surface-200 p-2 rounded">
{`#!/bin/sh
echo "=== SYSTEM DIAGNOSTIC ==="
echo "OS:      $(uname -s) $(uname -r) ($(uname -m))"
echo "Host:    $(hostname)"
echo "Memory:  $(free -h 2>/dev/null | awk '/Mem:/ {print $3 "/" $2}')"
echo "Disk:    $(df -h / | awk 'NR==2 {print $3 "/" $2 " (" $5 " used)"}')"`}
                  </pre>
                </div>

                {/* Tool 2: run_shell_command */}
                <div className="bg-surface-100 p-3 rounded-lg border border-border-subtle space-y-2">
                  <div className="text-xs text-emerald-400 font-semibold">
                    ⚡ [tool_call] run_shell_command &gt; chmod +x sysinfo.sh &amp;&amp; ./sysinfo.sh
                  </div>
                  <div className="bg-surface-200 p-3 rounded border border-border-subtle text-xs text-cyan-300 font-mono">
                    <div className="text-amber-300 font-bold mb-1">=== SYSTEM DIAGNOSTIC ===</div>
                    <div>OS:      Linux 6.12.9-arch1-1 (x86_64)</div>
                    <div>Host:    arch-workstation</div>
                    <div>Memory:  4.2Gi / 31.2Gi</div>
                    <div>Disk:    48G / 468G (11% used)</div>
                  </div>
                </div>

                <div className="text-emerald-400 text-xs flex items-center gap-1.5">
                  <span>✓</span> Task completed autonomously in 1.42s (0 user confirmations required).
                </div>
              </motion.div>
            )}

            {activeScenario === 'gemini' && (
              <motion.div
                key="gemini"
                initial={{ opacity: 0, y: 6 }}
                animate={{ opacity: 1, y: 0 }}
                exit={{ opacity: 0, y: -6 }}
                transition={{ duration: 0.2 }}
                className="space-y-4"
              >
                <div className="flex items-start gap-2">
                  <span className="text-cyan-400 font-bold select-none">&gt;</span>
                  <div>
                    <span className="text-gray-400">You: </span>
                    <span className="text-white font-semibold">find potential race conditions in src/file_lock.rs</span>
                  </div>
                </div>

                <div className="bg-surface-100/70 p-3 rounded-lg border border-border-subtle space-y-2">
                  <div className="flex items-center gap-2 text-xs text-violet-400 font-medium">
                    <Sparkles className="w-3.5 h-3.5" />
                    <span>Google Thought Signature [preserved across multi-turn tool schema]</span>
                  </div>
                  <p className="text-xs text-gray-400 italic">
                    "Analyzing mutex guard scopes in lock_path(). Checking if path normalization handles relative symlinks before acquiring the advisory file descriptor..."
                  </p>
                </div>

                <div className="bg-surface-100 p-3 rounded-lg border border-border-subtle text-xs space-y-1">
                  <div className="text-emerald-400 font-semibold">⚡ [tool_call] read_file &gt; src/file_lock.rs (L1-L45)</div>
                  <div className="text-gray-300">
                    File inspected. Mutex correctly uses canonicalized paths with <code className="text-cyan-300">fs::canonicalize</code> and non-blocking <code className="text-cyan-300">flock(LOCK_EX | LOCK_NB)</code>. No race condition detected.
                  </div>
                </div>
              </motion.div>
            )}

            {activeScenario === 'termux' && (
              <motion.div
                key="termux"
                initial={{ opacity: 0, y: 6 }}
                animate={{ opacity: 1, y: 0 }}
                exit={{ opacity: 0, y: -6 }}
                transition={{ duration: 0.2 }}
                className="max-w-md mx-auto bg-surface-100 p-4 rounded-xl border border-border-subtle space-y-3 text-xs"
              >
                <div className="text-center pb-2 border-b border-border-subtle text-gray-400 text-xs">
                  <span className="text-emerald-400 font-bold">COREX MOBILE TUI</span> · 52x28 (Termux Portrait)
                </div>

                <div className="text-gray-300 space-y-1">
                  <div>┌─ Model: <span className="text-cyan-400">Gemini 3.5 Flash</span></div>
                  <div>└─ Session: <span className="text-gray-400">#mobile-dev (2 turns)</span></div>
                </div>

                <div className="bg-surface-200 p-2.5 rounded border border-border-subtle text-gray-200">
                  <span className="text-cyan-400 font-bold">&gt;</span> edit crates/corex-core/src/types.rs
                  <div className="text-emerald-400 mt-1">✓ Screen tap opens software keyboard instantly</div>
                  <div className="text-gray-400 mt-0.5">(Terminal mouse capture auto-disabled on mobile portrait)</div>
                </div>

                <div className="h-6 bg-surface-200 rounded px-2 flex items-center justify-between text-gray-400">
                  <span>[ /help ] [ /model ] [ /yolo ]</span>
                  <span className="text-emerald-400">● YOLO</span>
                </div>
              </motion.div>
            )}

            {activeScenario === 'models' && (
              <motion.div
                key="models"
                initial={{ opacity: 0, y: 6 }}
                animate={{ opacity: 1, y: 0 }}
                exit={{ opacity: 0, y: -6 }}
                transition={{ duration: 0.2 }}
                className="space-y-3 text-xs"
              >
                <div className="text-cyan-400 font-semibold border-b border-border-subtle pb-2 flex items-center justify-between">
                  <span>/model — Provider &amp; Model Architecture Catalog</span>
                  <span className="text-gray-400">Esc to close · Enter to select</span>
                </div>

                <div className="grid grid-cols-1 sm:grid-cols-2 gap-2 text-xs">
                  <div className="bg-cyan-500/10 border border-cyan-500/40 p-2.5 rounded-lg">
                    <div className="flex items-center justify-between font-bold text-white">
                      <span>● Google (Gemini)</span>
                      <span className="text-cyan-400">ACTIVE</span>
                    </div>
                    <div className="text-gray-400 mt-1">gemini-3.5-flash-lite · AI Studio (Key saved)</div>
                  </div>

                  <div className="bg-surface-100 border border-border-subtle p-2.5 rounded-lg hover:border-border-hover">
                    <div className="flex items-center justify-between font-bold text-gray-200">
                      <span>○ DeepSeek</span>
                      <span className="text-emerald-400 font-mono">READY ($12.23)</span>
                    </div>
                    <div className="text-gray-400 mt-1">deepseek-reasoner (V3 / R1)</div>
                  </div>

                  <div className="bg-surface-100 border border-border-subtle p-2.5 rounded-lg hover:border-border-hover">
                    <div className="flex items-center justify-between font-bold text-gray-200">
                      <span>○ Anthropic Claude</span>
                      <span className="text-violet-400">READY</span>
                    </div>
                    <div className="text-gray-400 mt-1">claude-3-7-sonnet-20250219 (SSE messages)</div>
                  </div>

                  <div className="bg-surface-100 border border-border-subtle p-2.5 rounded-lg hover:border-border-hover">
                    <div className="flex items-center justify-between font-bold text-gray-200">
                      <span>○ Local Ollama / LLaMA</span>
                      <span className="text-amber-400 font-mono">$0.00 FREE</span>
                    </div>
                    <div className="text-gray-400 mt-1">http://127.0.0.1:11434 (Air-gapped)</div>
                  </div>
                </div>
              </motion.div>
            )}
          </AnimatePresence>
        </div>
      </div>
    </section>
  );
};
