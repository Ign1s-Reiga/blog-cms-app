import { CheckCircle2, FilePen, FileText, Image } from 'lucide-react';

// ─── Types ────────────────────────────────────────────────────────────────────

export interface Post {
  id: string;
  title: string;
  status: 'published' | 'draft';
  date: string;
  tags: string[];
  views?: number;
}

// ─── Mock data ────────────────────────────────────────────────────────────────

export const POSTS: Post[] = [
  {
    id: '1',
    title: 'Getting Started with Tauri and Next.js',
    status: 'published',
    date: '2026-04-10',
    tags: ['tauri', 'nextjs'],
    views: 1240,
  },
  {
    id: '2',
    title: 'Cloudflare R2 as a Blog Storage Backend',
    status: 'published',
    date: '2026-04-08',
    tags: ['cloudflare', 'storage'],
    views: 874,
  },
  {
    id: '3',
    title: 'Markdown Parsing Deep Dive',
    status: 'draft',
    date: '2026-04-11',
    tags: ['markdown'],
  },
  {
    id: '4',
    title: 'Building a CMS with Rust',
    status: 'draft',
    date: '2026-04-12',
    tags: ['rust', 'cms'],
  },
  {
    id: '5',
    title: 'Deploying Tauri Apps to Windows',
    status: 'published',
    date: '2026-04-06',
    tags: ['tauri', 'deployment'],
    views: 532,
  },
];

export const STATS = [
  { label: 'Total Posts', value: '5', Icon: FileText, delta: '+2 this week', positive: true },
  { label: 'Published', value: '3', Icon: CheckCircle2, delta: '60% of total', positive: true },
  { label: 'Drafts', value: '2', Icon: FilePen, delta: 'Pending review', positive: false },
  { label: 'Media Files', value: '18', Icon: Image, delta: '+3 this week', positive: true },
];
