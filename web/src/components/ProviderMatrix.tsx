import React from 'react';
import { Server, Key, CheckCircle2 } from 'lucide-react';

export const ProviderMatrix: React.FC = () => {
  const providers = [
    {
      name: 'Google Gemini',
      endpoint: '/v1beta/openai',
      models: 'gemini-3.5-flash-lite, gemini-3.8-flash',
      highlight: 'Thought Signatures & Delta Chunks',
      color: 'border-blue-500/30 bg-blue-500/5',
      textColor: 'text-blue-400',
    },
    {
      name: 'Anthropic Claude',
      endpoint: '/v1/messages',
      models: 'claude-3-7-sonnet, claude-3-5-haiku',
      highlight: 'Native SSE Adapter & Tool Mapping',
      color: 'border-violet-500/30 bg-violet-500/5',
      textColor: 'text-violet-400',
    },
    {
      name: 'DeepSeek',
      endpoint: 'api.deepseek.com',
      models: 'deepseek-chat (V3), deepseek-reasoner (R1)',
      highlight: 'Native Cost & Balance Telemetry',
      color: 'border-cyan-500/30 bg-cyan-500/5',
      textColor: 'text-cyan-400',
    },
    {
      name: 'OpenAI',
      endpoint: '/v1/chat/completions',
      models: 'gpt-4o, o1, o3-mini',
      highlight: 'Reasoning Effort & Temp Stripping',
      color: 'border-emerald-500/30 bg-emerald-500/5',
      textColor: 'text-emerald-400',
    },
    {
      name: 'Local LLMs (Air-gapped)',
      endpoint: '127.0.0.1:11434 (Ollama / llama.cpp)',
      models: 'Qwen 2.5 Coder, Llama 3.3, DeepSeek R1',
      highlight: '$0.00 API Cost & 100% Offline',
      color: 'border-amber-500/30 bg-amber-500/5',
      textColor: 'text-amber-400',
    },
    {
      name: 'Groq & GitHub Models',
      endpoint: 'groq.com / models.inference.ai.azure.com',
      models: 'Llama 3.3 70B Versatile, Mistral Large',
      highlight: '500+ tok/s Ultra Low Latency',
      color: 'border-rose-500/30 bg-rose-500/5',
      textColor: 'text-rose-400',
    },
  ];

  return (
    <section id="providers" className="py-16 md:py-24 max-w-7xl mx-auto px-4 sm:px-6 lg:px-8">
      <div className="text-center max-w-3xl mx-auto mb-14">
        <div className="inline-flex items-center gap-2 px-3 py-1 rounded-full bg-surface-100 border border-border-subtle text-xs font-mono text-cyan-300 mb-4">
          <Server className="w-3.5 h-3.5 text-cyan-400" />
          <span>Zero Vendor Lock-In</span>
        </div>
        <h2 className="text-3xl sm:text-4xl font-bold tracking-tight text-white mb-4 font-sans">
          One Engine. Every Major LLM Protocol.
        </h2>
        <p className="text-gray-400 text-base sm:text-lg font-normal">
          Save independent API keys per provider in <code className="text-cyan-300 font-mono text-sm bg-surface-100 px-1.5 py-0.5 rounded border border-border-subtle">~/.corex/settings.json</code> without configuration clobbering.
        </p>
      </div>

      <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-6">
        {providers.map((p, idx) => (
          <div
            key={idx}
            className={`p-5 rounded-xl border ${p.color} bg-surface-100/60 hover:bg-surface-100 transition-all glow-card flex flex-col justify-between`}
          >
            <div>
              <div className="flex items-center justify-between mb-3">
                <span className={`font-bold font-sans text-base ${p.textColor}`}>
                  {p.name}
                </span>
                <span className="text-[11px] font-mono text-gray-400 bg-surface-200 px-2 py-0.5 rounded border border-border-subtle">
                  {p.endpoint}
                </span>
              </div>

              <div className="text-xs font-mono text-gray-300 mb-2">
                <span className="text-gray-400">Models: </span>
                {p.models}
              </div>
            </div>

            <div className="mt-4 pt-3 border-t border-border-subtle/50 flex items-center justify-between text-xs">
              <span className="text-gray-400 flex items-center gap-1">
                <CheckCircle2 className="w-3.5 h-3.5 text-emerald-400" />
                {p.highlight}
              </span>
              <Key className="w-3 h-3 text-gray-400" />
            </div>
          </div>
        ))}
      </div>
    </section>
  );
};
