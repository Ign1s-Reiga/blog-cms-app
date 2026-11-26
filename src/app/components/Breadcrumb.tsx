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
  '/tags': 'Tags',
  '/media': 'Media',
  '/analytics': 'Analytics',
  '/settings': 'Settings',
};

export function Breadcrumb() {
  const pathname = usePathname();
  const label = LABELS[pathname] ?? 'Page';

  return (
    <BreadcrumbRoot className='text-ui'>
      <BreadcrumbList className='gap-[5px] sm:gap-[5px]'>
        <BreadcrumbItem className='hidden sm:flex text-muted-foreground font-medium'>blog-cms</BreadcrumbItem>
        <BreadcrumbSeparator className='hidden sm:block text-muted-foreground' />
        <BreadcrumbItem>
          <BreadcrumbPage className='font-semibold text-foreground'>{label}</BreadcrumbPage>
        </BreadcrumbItem>
      </BreadcrumbList>
    </BreadcrumbRoot>
  );
}
