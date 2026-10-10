import React from 'react';
import { Terminal, Shield } from 'lucide-react';
import { GithubIcon } from './GithubIcon';

export const Footer: React.FC = () => {
  return (
    <footer className="border-t border-border-subtle/70 bg-surface-300 py-12 text-xs font-mono text-gray-400">
      <div className="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8 flex flex-col sm:flex-row items-center justify-between gap-6">
        {/* Brand & Copyright */}
        <div className="flex items-center gap-3">
          <div className="w-7 h-7 rounded-md bg-surface-200 border border-border-subtle flex items-center justify-center text-cyan-400">
            <Terminal className="w-4 h-4" />
          </div>
          <div>
            <span className="text-white font-bold font-sans text-sm">Corex</span>
            <span className="text-gray-400 ml-2">© 2026 sluisr. All rights reserved.</span>
          </div>
        </div>

        {/* License & Attribution */}
        <div className="flex items-center gap-4 text-xs">
          <span className="flex items-center gap-1.5 px-2.5 py-1 rounded bg-surface-200 border border-border-subtle text-gray-300">
            <Shield className="w-3.5 h-3.5 text-cyan-400" />
            <span>AGPL-3.0 License</span>
          </span>

          <a
            href="https://github.com/sluisr/corex"
            target="_blank"
            rel="noopener noreferrer"
            className="flex items-center gap-1.5 text-gray-400 hover:text-white transition-colors"
          >
            <GithubIcon className="w-4 h-4" />
            <span>GitHub</span>
          </a>

          <a
            href="https://www.npmjs.com/package/@sluisr/corex"
            target="_blank"
            rel="noopener noreferrer"
            className="text-gray-400 hover:text-white transition-colors"
          >
            NPM
          </a>
        </div>
      </div>
    </footer>
  );
};
