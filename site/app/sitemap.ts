import type { MetadataRoute } from "next";
import { url } from "@/lib/site";

export default function sitemap(): MetadataRoute.Sitemap {
  return [{ url }, { url: `${url}/capcut-alternative` }];
}
