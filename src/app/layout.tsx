import type { Metadata } from "next";
import "./globals.css";
import { ThemeProvider } from "next-themes";
import { Header } from "./components/Header";
import { Sidebar } from "./components/Sidebar";
import { SidebarProvider } from "./components/SidebarProvider";

export const metadata: Metadata = {
  title: "Blog CMS",
  description: "Manage your blog content",
};

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en" suppressHydrationWarning>
      <body>
        <ThemeProvider attribute="class" defaultTheme="light" disableTransitionOnChange>
          <SidebarProvider>
            <div className="flex h-screen overflow-hidden bg-zinc-50 dark:bg-[#0a0a0a] text-zinc-900 dark:text-zinc-100 antialiased">
              <Sidebar />
              <div className="flex-1 flex flex-col min-w-0">
                <Header />
                {children}
              </div>
            </div>
          </SidebarProvider>
        </ThemeProvider>
      </body>
    </html>
  );
}
