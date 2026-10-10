import React from 'react';
import { Navbar } from './components/Navbar';
import { Hero } from './components/Hero';
import { TerminalDemo } from './components/TerminalDemo';
import { FeaturesGrid } from './components/FeaturesGrid';
import { ProviderMatrix } from './components/ProviderMatrix';
import { InstallSection } from './components/InstallSection';
import { Footer } from './components/Footer';

export const App: React.FC = () => {
  return (
    <div className="min-h-screen bg-background text-gray-100 flex flex-col relative bg-grid-pattern">
      <Navbar />
      <main className="flex-grow">
        <Hero />
        <TerminalDemo />
        <FeaturesGrid />
        <ProviderMatrix />
        <InstallSection />
      </main>
      <Footer />
    </div>
  );
};

export default App;
