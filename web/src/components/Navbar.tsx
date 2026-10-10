import React from 'react';
import { Terminal, Star, Sparkles, ExternalLink } from 'lucide-react';
import { GithubIcon } from './GithubIcon';

export const Navbar: React.FC = () => {
  return (
    <header className="sticky top-0 z-50 backdrop-blur-xl bg-background/80 border-b border-border-subtle/60">
      <div className="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8 h-16 flex items-center justify-between">
        {/* Brand */}
        <div className="flex items-center gap-3">
          <div className="w-9 h-9 rounded-lg bg-gradient-to-br from-cyan-500/20 to-blue-600/20 border border-cyan-500/30 flex items-center justify-center text-cyan-400 font-mono font-bold shadow-sm shadow-cyan-500/10">
            <Terminal className="w-5 h-5" />
          </div>
          <div className="flex items-center gap-2">
            <span className="font-bold text-lg tracking-tight text-white font-mono">
              corex
            </span>
            <span className="text-xs px-2 py-0.5 rounded-full bg-surface-50 border border-border-subtle text-cyan-400 font-mono font-medium">
              v0.5.0
            </span>
          </div>
        </div>

        {/* Navigation Links */}
        <nav className="hidden md:flex items-center gap-8 text-sm font-medium text-gray-400">
          <a href="#features" className="hover:text-white transition-colors">Features</a>
          <a href="#demo" className="hover:text-white transition-colors">Terminal Demo</a>
          <a href="#providers" className="hover:text-white transition-colors">Providers</a>
          <a href="#install" className="hover:text-white transition-colors">Installation</a>
        </nav>

        {/* Action Buttons */}
        <div className="flex items-center gap-3">
          <a
            href="https://github.com/sluisr/corex"
            target="_blank"
            rel="noopener noreferrer"
            className="flex items-center gap-2 px-3.5 py-1.5 rounded-lg bg-surface-100 hover:bg-surface-50 border border-border-subtle hover:border-border-hover text-sm font-medium text-gray-200 transition-all shadow-sm"
          >
            <GithubIcon className="w-4 h-4 text-gray-400" />
            <span>GitHub</span>
            <span className="hidden sm:inline-flex items-center gap-1 text-xs text-amber-400 font-mono pl-1 border-l border-border-subtle">
              <Star className="w-3 h-3 fill-amber-400" />
              <span>Star</span>
            </span>
          </a>

          <a
            href="https://www.npmjs.com/package/@sluisr/corex"
            target="_blank"
            rel="noopener noreferrer"
            className="hidden sm:flex items-center gap-1.5 px-3 py-1.5 rounded-lg bg-cyan-500/10 hover:bg-cyan-500/20 border border-cyan-500/30 text-cyan-300 text-xs font-mono font-medium transition-all"
          >
            <Sparkles className="w-3.5 h-3.5" />
            <span>NPM Registry</span>
            <ExternalLink className="w-3 h-3 opacity-60" />
          </a>
        </div>
      </div>
    </header>
  );
};
