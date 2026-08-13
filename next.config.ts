import type { NextConfig } from 'next';

const nextConfig: NextConfig = {
  reactStrictMode: true,
  // Tauri loads the frontend from a static bundle (see `frontendDist: "../out"`
  // in src-tauri/tauri.conf.json), so Next.js must emit a static export.
  output: 'export',
  images: { unoptimized: true },
};

export default nextConfig;
