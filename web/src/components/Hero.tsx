import React, { useState } from 'react';
import { Copy, Check, Shield, Cpu, Smartphone } from 'lucide-react';
import { motion } from 'framer-motion';

export const Hero: React.FC = () => {
  const [activeTab, setActiveTab] = useState<'curl' | 'npm' | 'npx'>('curl');
  const [copied, setCopied] = useState(false);

  const installCommands = {
    curl: 'curl -fsSL https://raw.githubusercontent.com/sluisr/corex/main/install.sh | sh',
    npm: 'npm install -g @sluisr/corex',
    npx: 'npx @sluisr/corex',
  };

  const currentCommand = installCommands[activeTab];

  const handleCopy = () => {
    navigator.clipboard.writeText(currentCommand);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  return (
    <section className="relative pt-16 pb-20 md:pt-24 md:pb-28 overflow-hidden">
      {/* Glow gradient backdrop */}
      <div className="absolute top-1/2 left-1/2 -translate-x-1/2 -translate-y-1/2 w-[600px] h-[350px] bg-gradient-to-tr from-cyan-600/15 via-indigo-600/10 to-transparent blur-[120px] pointer-events-none -z-10" />

      <div className="max-w-5xl mx-auto px-4 sm:px-6 lg:px-8 text-center">
        {/* Top Badges */}
        <motion.div 
          initial={{ opacity: 0, y: -10 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ duration: 0.5 }}
          className="inline-flex items-center gap-2 px-3.5 py-1.5 rounded-full bg-surface-100 border border-border-subtle text-xs font-mono text-gray-300 mb-8 shadow-inner"
        >
          <span className="flex h-2 w-2 rounded-full bg-emerald-400 animate-pulse" />
          <span className="text-gray-400">Corex 0.5.0 Release</span>
          <span className="text-gray-600">|</span>
          <span className="text-cyan-400 font-medium">AGPL-3.0 Open Source</span>
        </motion.div>

        {/* Headline */}
        <motion.h1 
          initial={{ opacity: 0, y: 15 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ duration: 0.6, delay: 0.1 }}
          className="text-4xl sm:text-6xl lg:text-7xl font-bold tracking-tight text-white mb-6 leading-[1.1] font-sans"
        >
          The Autonomous AI Agent <br className="hidden sm:inline" />
          <span className="bg-gradient-to-r from-cyan-400 via-sky-300 to-indigo-400 bg-clip-text text-transparent">
            Built in Rust for Developers.
          </span>
        </motion.h1>

        {/* Subtitle */}
        <motion.p 
          initial={{ opacity: 0, y: 15 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ duration: 0.6, delay: 0.2 }}
          className="text-lg sm:text-xl text-gray-400 max-w-3xl mx-auto mb-10 leading-relaxed font-normal"
        >
          Blazingly fast, multi-provider terminal coding assistant. Native support for Gemini 3.x, 
          Anthropic Claude, OpenAI, DeepSeek, local LLMs, and fully autonomous execution on 
          Linux, macOS, Windows & Android Termux.
        </motion.p>

        {/* Interactive Install Command Box */}
        <motion.div 
          initial={{ opacity: 0, scale: 0.98 }}
          animate={{ opacity: 1, scale: 1 }}
          transition={{ duration: 0.5, delay: 0.3 }}
          className="max-w-2xl mx-auto mb-10 text-left"
        >
          {/* Tab switches */}
          <div className="flex items-center gap-1 mb-2 px-1">
            <button
              onClick={() => setActiveTab('curl')}
              className={`px-3 py-1 rounded-md text-xs font-mono font-medium transition-all ${
                activeTab === 'curl'
                  ? 'bg-surface-100 text-cyan-300 border border-border-subtle'
                  : 'text-gray-500 hover:text-gray-300'
              }`}
            >
              curl (sh)
            </button>
            <button
              onClick={() => setActiveTab('npm')}
              className={`px-3 py-1 rounded-md text-xs font-mono font-medium transition-all ${
                activeTab === 'npm'
                  ? 'bg-surface-100 text-cyan-300 border border-border-subtle'
                  : 'text-gray-500 hover:text-gray-300'
              }`}
            >
              npm (global)
            </button>
            <button
              onClick={() => setActiveTab('npx')}
              className={`px-3 py-1 rounded-md text-xs font-mono font-medium transition-all ${
                activeTab === 'npx'
                  ? 'bg-surface-100 text-cyan-300 border border-border-subtle'
                  : 'text-gray-500 hover:text-gray-300'
              }`}
            >
              npx (instant)
            </button>
          </div>

          {/* Terminal Command bar */}
          <div className="relative flex items-center justify-between p-3.5 sm:p-4 rounded-xl bg-surface-100/90 border border-border-subtle hover:border-border-hover transition-all group glow-card">
            <div className="flex items-center gap-3 overflow-x-auto scrollbar-none mr-2 font-mono text-sm sm:text-base">
              <span className="text-cyan-400 select-none font-bold">$</span>
              <code className="text-gray-200 whitespace-nowrap">{currentCommand}</code>
            </div>
            
            <button
              onClick={handleCopy}
              className="flex items-center gap-1.5 px-3 py-1.5 rounded-lg bg-surface-50 hover:bg-surface-200 border border-border-subtle text-xs font-mono text-gray-300 hover:text-white transition-all flex-shrink-0"
              title="Copy to clipboard"
            >
              {copied ? (
                <>
                  <Check className="w-3.5 h-3.5 text-emerald-400" />
                  <span className="text-emerald-400">Copied!</span>
                </>
              ) : (
                <>
                  <Copy className="w-3.5 h-3.5 text-gray-400" />
                  <span>Copy</span>
                </>
              )}
            </button>
          </div>
        </motion.div>

        {/* Feature Highlights Pills */}
        <motion.div 
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          transition={{ duration: 0.6, delay: 0.4 }}
          className="flex flex-wrap items-center justify-center gap-6 text-xs sm:text-sm text-gray-400 font-mono"
        >
          <div className="flex items-center gap-2">
            <Cpu className="w-4 h-4 text-cyan-400" />
            <span>&lt; 5ms Native Rust Boot</span>
          </div>
          <div className="flex items-center gap-2">
            <Smartphone className="w-4 h-4 text-emerald-400" />
            <span>Android Termux Ready</span>
          </div>
          <div className="flex items-center gap-2">
            <Shield className="w-4 h-4 text-indigo-400" />
            <span>YOLO Sandbox with Protected Paths</span>
          </div>
        </motion.div>
      </div>
    </section>
  );
};
