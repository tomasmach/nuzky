import type { MetadataRoute } from "next";
import { url } from "@/lib/site";

export default function robots(): MetadataRoute.Robots {
  return { rules: { userAgent: "*", allow: "/" }, sitemap: `${url}/sitemap.xml` };
}
