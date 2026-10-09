import Image from "next/image";
import { maker, makerNote } from "@/lib/content";

// Why Nuzky exists, told by the person who builds it. The photo carries the section, so nothing around it decorates.
export function Maker() {
  return (
    <section id="why" className="mx-auto w-full max-w-[1040px] scroll-mt-10 px-5 pt-32 sm:pt-[180px]">
      <div className="flex flex-col gap-10 md:flex-row md:items-start md:gap-16">
        <figure className="flex shrink-0 items-center gap-4 md:sticky md:top-12 md:w-[300px] md:flex-col md:items-start md:gap-5">
          <Image
            src={maker.photo}
            alt=""
            width={300}
            height={300}
            sizes="(min-width: 768px) 300px, 72px"
            className="size-[72px] rounded-full object-cover ring-1 ring-white/10 md:size-[300px] md:rounded-[28px]"
          />
          <figcaption className="flex flex-col gap-0.5">
            <span className="text-[16px] font-semibold">{maker.name}</span>
            <span className="text-[14px] text-muted">{maker.role}</span>
          </figcaption>
        </figure>

        <div className="flex max-w-[620px] flex-col">
          <h2 className="heading-lg">
            Two hours of captions and ums.
            <br />
            <span className="text-subtle">For every single video.</span>
          </h2>
          <div className="mt-8 flex flex-col gap-5 text-[17px] leading-[1.6] text-fg/75 sm:text-[19px]">
            {makerNote.map((p) => (
              <p key={p}>{p}</p>
            ))}
          </div>
          <p className="mt-8 text-[14px] text-muted">
            Nuzky comes from <span lang="cs">nůžky</span>, Czech for scissors.
          </p>
        </div>
      </div>
    </section>
  );
}
