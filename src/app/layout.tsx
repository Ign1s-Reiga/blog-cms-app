import type { Metadata } from "next";
import "./globals.css";
import { ThemeProvider } from "next-themes";
import { AuthGate } from "@/components/AuthGate";
import { Header } from "@/components/Header";
import { Sidebar } from "@/components/Sidebar";
import { SidebarProvider } from "@/components/SidebarProvider";
import { Geist } from "next/font/google";
import { cn } from "@/lib/utils";

const geist = Geist({subsets:['latin'],variable:'--font-sans'});

export const metadata: Metadata = {
  title: "Blog CMS",
  description: "Manage your blog content",
};

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en" suppressHydrationWarning className={cn("font-sans", geist.variable)}>
      <body>
        <ThemeProvider attribute="class" defaultTheme="light" disableTransitionOnChange>
          <SidebarProvider>
            <AuthGate>
              <div className="flex h-screen overflow-hidden bg-zinc-50 dark:bg-[#0a0a0a] text-zinc-900 dark:text-zinc-100 antialiased">
                <Sidebar />
                <div className="flex-1 flex flex-col min-w-0">
                  <Header />
                  {children}
                </div>
              </div>
            </AuthGate>
          </SidebarProvider>
        </ThemeProvider>
      </body>
    </html>
  );
}
