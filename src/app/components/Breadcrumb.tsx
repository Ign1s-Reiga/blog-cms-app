'use client';

import { usePathname } from 'next/navigation';
import {
  Breadcrumb as BreadcrumbRoot,
  BreadcrumbItem,
  BreadcrumbList,
  BreadcrumbPage,
  BreadcrumbSeparator,
} from '@/components/ui/breadcrumb';

const LABELS: Record<string, string> = {
  '/': 'Dashboard',
  '/posts': 'Posts',
  '/posts/new': 'New Post',
  '/series': 'Series',
  '/media': 'Media',
  '/analytics': 'Analytics',
  '/settings': 'Settings',
};

export function Breadcrumb() {
  const pathname = usePathname();
  const label = LABELS[pathname] ?? 'Page';

  return (
    <BreadcrumbRoot className='text-[13px]'>
      <BreadcrumbList className='gap-[5px] sm:gap-[5px]'>
        <BreadcrumbItem className='hidden sm:flex text-zinc-400 dark:text-zinc-600 font-medium'>
          blog-cms
        </BreadcrumbItem>
        <BreadcrumbSeparator className='hidden sm:block text-zinc-300 dark:text-zinc-700' />
        <BreadcrumbItem>
          <BreadcrumbPage className='font-semibold text-zinc-800 dark:text-zinc-200'>{label}</BreadcrumbPage>
        </BreadcrumbItem>
      </BreadcrumbList>
    </BreadcrumbRoot>
  );
}
