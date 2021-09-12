import React, { ReactNode } from "react";

export type CardHeaderProps = {
    slug: string;
    title: string;
    children?: ReactNode;
};

export function CardHeader({ slug, title, children }: CardHeaderProps) {
    return (
      <div id={slug} className="card-header">
          <h3 className="card-header-title">
            <span className="fe fe-link mr-2 font-size-sm"></span>
        <a href={`#${slug}`} className="header-link">
            {title}
        </a>
            </h3>
        {children}
      </div>
    );
}
