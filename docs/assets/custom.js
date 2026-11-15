(() => {
    const root = typeof path_to_root === "string" ? path_to_root : "";
    const repo = "https://github.com/Valescent/engine";
    const byId = (name) => document.getElementById(name) || document.getElementById(`mdbook-${name}`);
    const themeButton = (name) => document.getElementById(name) || document.getElementById(`mdbook-theme-${name}`);
    const clamp = (value, min, max) => Math.min(max, Math.max(min, value));

    const linkTitle = (link) => {
        const copy = link.cloneNode(true);
        copy.querySelectorAll("strong").forEach((node) => node.remove());
        return copy.textContent.replace(/\s+/g, " ").trim();
    };

    const currentFile = () => {
        const parts = window.location.pathname.split("/");
        return decodeURIComponent(parts[parts.length - 1] || "index.html");
    };

    const initMenu = () => {
        const menu = byId("menu-bar");
        if (!menu) return;

        const title = menu.querySelector(".menu-title");
        if (title) {
            let brand = title;
            if (title.tagName === "H1") {
                brand = document.createElement("a");
                brand.className = "menu-title";
                brand.href = root + "index.html";
                brand.innerHTML = title.innerHTML;
                title.replaceWith(brand);
            }
            if (!brand.querySelector(".brand-state")) {
                const state = document.createElement("span");
                state.className = "brand-state";
                state.textContent = "pre-alpha";
                brand.append(state);
            }
            brand.addEventListener("click", () => {
                window.location.href = root + "index.html";
            });
        }

        const right = menu.querySelector(".right-buttons");
        if (right && !right.querySelector(".menu-link")) {
            const roadmap = document.createElement("a");
            roadmap.className = "menu-link";
            roadmap.href = root + "roadmap.html";
            roadmap.textContent = "Roadmap";
            if (currentFile() === "roadmap.html") roadmap.classList.add("is-active");
            right.prepend(roadmap);
        }

        if (!byId("read-progress")) {
            const progress = document.createElement("div");
            progress.id = "read-progress";
            menu.append(progress);
        }

        const themeList = byId("theme-list");
        if (themeList) {
            ["rust", "navy", "ayu"].forEach((id) => {
                const button = themeButton(id);
                if (button) button.closest("li")?.remove();
            });
            const labels = {
                default_theme: "System",
                light: "Paper",
                coal: "Night",
            };
            Object.entries(labels).forEach(([id, label]) => {
                const button = themeButton(id);
                if (button) button.textContent = label;
            });
        }

        const search = byId("searchbar");
        if (search) search.placeholder = "Search the documentation";
    };

    const initSidebar = () => {
        const sidebar = byId("sidebar");
        if (!sidebar || sidebar.querySelector(".sidebar-head")) return;

        const head = document.createElement("div");
        head.className = "sidebar-head";
        head.innerHTML = [
            '<span class="sidebar-head-label">Field index</span>',
            '<span class="sidebar-head-status"><span class="sidebar-head-dot"></span>pre-alpha · 0.0.2 shipped</span>',
            '<button class="sidebar-close" type="button" aria-label="Close table of contents"><span></span></button>',
        ].join("");
        sidebar.prepend(head);
        sidebar.classList.add("has-side-head");

        const scrim = document.createElement("div");
        scrim.className = "sidebar-scrim";
        scrim.setAttribute("aria-hidden", "true");
        sidebar.insertAdjacentElement("afterend", scrim);

        const anchor = byId("sidebar-toggle-anchor");
        const closeSidebar = () => {
            if (!anchor) return;
            anchor.checked = false;
            anchor.dispatchEvent(new Event("change", { bubbles: true }));
        };
        head.querySelector(".sidebar-close")?.addEventListener("click", closeSidebar);
        scrim.addEventListener("click", closeSidebar);

        const foot = document.createElement("div");
        foot.className = "sidebar-foot";
        foot.innerHTML = [
            "<span>PolyForm Shield</span>",
            `<a href="${repo}#licence" rel="noopener">Source</a>`,
        ].join("");
        sidebar.append(foot);
        sidebar.classList.add("has-side-foot");
    };

    const sidebarReady = (callback) => {
        const sidebar = byId("sidebar");
        if (!sidebar) return;
        if (sidebar.querySelector(".chapter a")) {
            callback(sidebar);
            return;
        }
        const observer = new MutationObserver(() => {
            if (sidebar.querySelector(".chapter a")) {
                observer.disconnect();
                callback(sidebar);
            }
        });
        observer.observe(sidebar, { childList: true, subtree: true });
        window.setTimeout(() => observer.disconnect(), 4000);
    };

    const activeSidebarLink = (sidebar) => {
        const links = Array.from(sidebar.querySelectorAll(".chapter a[href]"));
        const here = currentFile();
        return (
            links.find((link) => link.classList.contains("active")) ||
            links.find((link) => {
                const href = link.getAttribute("href") || "";
                return decodeURIComponent(href.split("/").pop() || "") === here;
            }) ||
            null
        );
    };

    const initBreadcrumbs = (sidebar) => {
        const main = byId("content")?.querySelector("main");
        const active = activeSidebarLink(sidebar);
        if (!main || !active || document.querySelector(".hero")) return;
        if (main.querySelector(".crumbs")) return;

        const part = (() => {
            let item = active.closest("li.chapter-item");
            while (item) {
                let previous = item.previousElementSibling;
                while (previous) {
                    if (previous.classList.contains("part-title")) return previous.textContent.trim();
                    previous = previous.previousElementSibling;
                }
                item = item.parentElement?.closest("li.chapter-item") || null;
            }
            return "";
        })();

        const heading = main.querySelector("h1");
        const page = heading ? heading.textContent.trim() : "";
        const crumbs = document.createElement("nav");
        crumbs.className = "crumbs";
        crumbs.setAttribute("aria-label", "Breadcrumb");

        const home = document.createElement("a");
        home.href = root + "index.html";
        home.textContent = "Docs";
        crumbs.append(home);

        if (part && (!page || part.toLowerCase() !== page.toLowerCase())) {
            const separator = document.createElement("span");
            separator.className = "crumbs-sep";
            separator.textContent = "/";
            const partLabel = document.createElement("span");
            partLabel.textContent = part;
            crumbs.append(separator, partLabel);
        }

        if (page) {
            const separator = document.createElement("span");
            separator.className = "crumbs-sep";
            separator.textContent = "/";
            const current = document.createElement("span");
            current.className = "crumbs-current";
            current.textContent = page;
            crumbs.append(separator, current);
        }

        main.prepend(crumbs);
    };

    const buildToc = (headings) => {
        const list = document.createElement("ol");
        let currentSub = null;

        headings.forEach((heading) => {
            const link = document.createElement("a");
            link.href = `#${heading.id}`;
            link.textContent = heading.textContent.trim();
            link.dataset.target = heading.id;

            const item = document.createElement("li");
            item.append(link);

            if (heading.tagName === "H3" && currentSub) {
                currentSub.append(item);
            } else if (heading.tagName === "H3") {
                list.append(item);
            } else {
                list.append(item);
                currentSub = document.createElement("ol");
                item.append(currentSub);
            }
        });

        return list;
    };

    const initToc = () => {
        const content = byId("content");
        const main = content?.querySelector("main");
        if (!content || !main || document.querySelector(".hero")) return;

        const headings = Array.from(main.querySelectorAll("h2[id], h3[id]")).filter(
            (heading) => !heading.closest("blockquote") && !heading.closest(".closing")
        );
        if (headings.length < 2) return;

        const levelTwos = headings.filter((heading) => heading.tagName === "H2");
        const levelThrees = headings.filter((heading) => heading.tagName === "H3");
        const visibleHeadings =
            levelTwos.length > 0 && levelTwos.length + levelThrees.length > 12
                ? levelTwos
                : headings;
        if (visibleHeadings.length < 2) return;

        if (!content.querySelector(".page-toc")) {
            const aside = document.createElement("aside");
            aside.className = "page-toc";
            aside.setAttribute("aria-label", "On this page");

            const label = document.createElement("p");
            label.className = "toc-label";
            label.textContent = "On this page";
            aside.append(label, buildToc(visibleHeadings));

            const nav = content.querySelector(".nav-wrapper");
            content.insertBefore(aside, nav);
        }

        const h1 = main.querySelector("h1");
        if (h1 && !main.querySelector(".toc-inline")) {
            const details = document.createElement("details");
            details.className = "toc-inline";
            const summary = document.createElement("summary");
            summary.textContent = "On this page";
            details.append(summary, buildToc(visibleHeadings));
            h1.insertAdjacentElement("afterend", details);
        }

        const links = Array.from(content.querySelectorAll(".page-toc a[data-target]"));

        let scheduled = false;
        const updateActive = () => {
            scheduled = false;
            const threshold = 140;
            let current = visibleHeadings[0];
            visibleHeadings.forEach((heading) => {
                if (heading.getBoundingClientRect().top <= threshold) current = heading;
            });
            links.forEach((link) => {
                link.classList.toggle("is-active", link.dataset.target === current.id);
            });
        };
        const scheduleActive = () => {
            if (scheduled) return;
            scheduled = true;
            requestAnimationFrame(updateActive);
        };
        window.addEventListener("scroll", scheduleActive, { passive: true });
        window.addEventListener("resize", scheduleActive);
        updateActive();
    };

    const initPager = (sidebar) => {
        const main = byId("content")?.querySelector("main");
        if (!main || main.querySelector(".page-pager")) return;

        const links = Array.from(sidebar.querySelectorAll(".chapter a[href]")).filter(
            (link) => !link.closest(".part-title")
        );
        const active = activeSidebarLink(sidebar);
        const index = active ? links.indexOf(active) : -1;
        const previous = index > 0 ? links[index - 1] : null;
        const next = index >= 0 && index < links.length - 1 ? links[index + 1] : null;

        if (previous || next) {
            const pager = document.createElement("nav");
            pager.className = "page-pager";
            pager.setAttribute("aria-label", "Page navigation");

            const make = (link, direction, label) => {
                if (!link) return null;
                const anchor = document.createElement("a");
                anchor.className = `is-${direction}`;
                anchor.href = link.getAttribute("href");
                anchor.innerHTML = `<span class="pager-label">${label}</span><span class="pager-title"></span>`;
                anchor.querySelector(".pager-title").textContent = linkTitle(link);
                return anchor;
            };

            const prevLink = make(previous, "prev", "Previous");
            const nextLink = make(next, "next", "Next");
            if (prevLink) pager.append(prevLink);
            if (nextLink) pager.append(nextLink);
            main.append(pager);
        }

        const edit = document.querySelector('.menu-bar a[rel="edit"], #menu-bar a[rel="edit"], #mdbook-menu-bar a[rel="edit"]');
        if (document.querySelector(".hero")) return;
        const meta = document.createElement("div");
        meta.className = "page-meta";
        meta.innerHTML = [
            "<span>Valescent Engine · pre-alpha</span>",
            '<span class="page-meta-links">',
            edit
                ? `<a href="${edit.href}" rel="noopener">Edit this page</a>`
                : `<a href="${repo}" rel="noopener">Edit on GitHub</a>`,
            '<a href="#top">Back to top</a>',
            "</span>",
        ].join("");
        main.append(meta);
    };

    const initCodeLabels = () => {
        document.querySelectorAll(".content pre > code").forEach((code) => {
            const pre = code.parentElement;
            if (!pre || pre.dataset.lang) return;
            const match = Array.from(code.classList).find((name) => name.startsWith("language-"));
            const language = match ? match.replace("language-", "") : "text";
            const labels = {
                console: "shell",
                sh: "shell",
                bash: "shell",
                rs: "rust",
                js: "javascript",
                html: "html",
                toml: "toml",
                wgsl: "wgsl",
            };
            pre.dataset.lang = labels[language] || language;
        });
    };

    const initProgress = () => {
        const bar = byId("read-progress");
        if (!bar) return;
        const update = () => {
            const scrollable = document.documentElement.scrollHeight - window.innerHeight;
            const ratio = scrollable > 0 ? clamp(window.scrollY / scrollable, 0, 1) : 0;
            bar.style.transform = `scaleX(${ratio})`;
        };
        update();
        window.addEventListener("scroll", update, { passive: true });
        window.addEventListener("resize", update);
    };

    const initThemeSync = () => {
        const meta = document.querySelector('meta[name="theme-color"]');
        const sync = () => {
            if (meta) {
                const bg = getComputedStyle(document.documentElement).getPropertyValue("--bg").trim();
                if (bg) meta.setAttribute("content", bg);
            }
            window.dispatchEvent(new CustomEvent("valescent:theme"));
        };
        new MutationObserver(sync).observe(document.documentElement, {
            attributes: true,
            attributeFilter: ["class"],
        });
        sync();
    };

    const initVoxelPlate = () => {
        const stage = document.querySelector(".survey-stage");
        const canvas = byId("voxel-plate");
        if (!stage || !canvas) return;

        const context = canvas.getContext("2d");
        if (!context) return;

        document.querySelector(".plate-fallback")?.remove();
        canvas.hidden = false;

        const yawReadout = document.querySelector("[data-yaw]");
        const reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)");
        const size = { width: 0, height: 0, dpr: 1 };
        const N = 6;
        const baseYaw = 0.48;
        let yaw = baseYaw;
        let targetYaw = baseYaw;
        let idleYaw = baseYaw;
        let idleStart = performance.now();
        let dragging = false;
        let lastX = 0;
        let visible = true;
        let lastFrame = 0;
        let animation = 0;
        let palette = {};

        const readPalette = () => {
            const styles = getComputedStyle(document.documentElement);
            const value = (name) => styles.getPropertyValue(name).trim();
            return {
                grid: value("--scene-grid"),
                axis: value("--scene-axis"),
                top: [
                    value("--scene-top-1"),
                    value("--scene-top-2"),
                    value("--scene-top-3"),
                    value("--scene-top-4"),
                ],
                sideX: [value("--scene-side-x"), value("--scene-side-x-2")],
                sideZ: [value("--scene-side-z"), value("--scene-side-z-2")],
                edge: value("--scene-edge"),
                accent: value("--scene-accent"),
                accentX: value("--scene-accent-x"),
                accentZ: value("--scene-accent-z"),
                label: value("--scene-label"),
            };
        };

        const heightAt = (x, z) => {
            const radius = Math.sqrt(x * x + z * z);
            let height =
                2.4 +
                1.25 * Math.sin(x * 0.58 + 0.7) +
                0.9 * Math.cos(z * 0.51 - 0.4) +
                0.55 * Math.sin((x + z) * 0.42 + 1.3);
            const falloff = clamp((radius - 3.2) / 2.6, 0, 1);
            height = height * (1 - falloff) - falloff * 2.2;
            height = Math.max(0, Math.min(4, Math.floor(height + 0.2)));
            if (x === 0 && z === 0) height += 1;
            return height;
        };

        const cubes = [];
        for (let x = -N; x <= N; x += 1) {
            for (let z = -N; z <= N; z += 1) {
                const height = heightAt(x, z);
                for (let y = 0; y < height; y += 1) {
                    cubes.push({
                        x,
                        y,
                        z,
                        accent: x === 0 && z === 0 && y === height - 1,
                    });
                }
            }
        }

        const resize = () => {
            const rect = stage.getBoundingClientRect();
            size.width = Math.max(1, rect.width);
            size.height = Math.max(1, rect.height);
            size.dpr = Math.min(window.devicePixelRatio || 1, 2);
            canvas.width = Math.round(size.width * size.dpr);
            canvas.height = Math.round(size.height * size.dpr);
            canvas.style.width = `${size.width}px`;
            canvas.style.height = `${size.height}px`;
        };

        const draw = () => {
            if (!palette.grid) palette = readPalette();
            const { width, height, dpr } = size;
            if (!width || !height) return;

            const cos = Math.cos(yaw);
            const sin = Math.sin(yaw);
            const project = (x, y, z) => {
                const rx = x * cos - z * sin;
                const rz = x * sin + z * cos;
                return [(rx - rz) * 0.8660254, (rx + rz) * 0.5 - y * 0.9];
            };
            const depthOf = (cube) => {
                const x = cube.x + 0.5;
                const z = cube.z + 0.5;
                return (x * cos - z * sin) + (x * sin + z * cos);
            };

            const sorted = cubes.slice().sort((a, b) => {
                const depth = depthOf(a) - depthOf(b);
                return depth !== 0 ? depth : a.y - b.y;
            });

            let minX = Infinity;
            let maxX = -Infinity;
            let minY = Infinity;
            let maxY = -Infinity;
            const include = (point) => {
                if (point[0] < minX) minX = point[0];
                if (point[0] > maxX) maxX = point[0];
                if (point[1] < minY) minY = point[1];
                if (point[1] > maxY) maxY = point[1];
            };

            const gridMin = -N - 0.6;
            const gridMax = N + 0.6;
            [
                [gridMin, 0, gridMin],
                [gridMax, 0, gridMin],
                [gridMax, 0, gridMax],
                [gridMin, 0, gridMax],
            ].forEach((point) => include(project(...point)));

            sorted.forEach((cube) => {
                include(project(cube.x, cube.y, cube.z));
                include(project(cube.x + 1, cube.y + 1, cube.z + 1));
            });

            const padding = Math.min(width, height) * 0.065;
            const scale = Math.min(
                (width - padding * 2) / Math.max(1, maxX - minX),
                (height - padding * 2) / Math.max(1, maxY - minY)
            );
            const offsetX = (width - (maxX - minX) * scale) / 2 - minX * scale;
            const offsetY = (height - (maxY - minY) * scale) / 2 - minY * scale;

            context.setTransform(dpr, 0, 0, dpr, 0, 0);
            context.clearRect(0, 0, width, height);
            context.save();
            context.translate(offsetX, offsetY);
            context.scale(scale, scale);
            context.lineJoin = "round";
            context.lineCap = "square";

            const hairline = 1 / scale;
            context.strokeStyle = palette.grid;
            context.lineWidth = hairline;
            context.beginPath();
            for (let line = gridMin; line <= gridMax; line += 1) {
                const a = project(line, 0, gridMin);
                const b = project(line, 0, gridMax);
                const c = project(gridMin, 0, line);
                const d = project(gridMax, 0, line);
                context.moveTo(a[0], a[1]);
                context.lineTo(b[0], b[1]);
                context.moveTo(c[0], c[1]);
                context.lineTo(d[0], d[1]);
            }
            context.stroke();

            context.fillStyle = palette.label;
            context.font = `${1.05 / scale}px "Spline Sans Mono", monospace`;
            context.fillText("x", ...project(gridMax + 0.35, 0, 0));
            context.fillText("z", ...project(0, 0, gridMax + 0.35));

            const drawFace = (points, fill) => {
                context.beginPath();
                context.moveTo(points[0][0], points[0][1]);
                for (let index = 1; index < points.length; index += 1) {
                    context.lineTo(points[index][0], points[index][1]);
                }
                context.closePath();
                context.fillStyle = fill;
                context.fill();
                context.strokeStyle = palette.edge;
                context.lineWidth = hairline;
                context.stroke();
            };

            sorted.forEach((cube) => {
                const x = cube.x;
                const y = cube.y;
                const z = cube.z;
                const top = cube.accent ? palette.accent : palette.top[(x + z + N * 2) % palette.top.length];
                const sideX = cube.accent
                    ? palette.accentX
                    : palette.sideX[y % palette.sideX.length];
                const sideZ = cube.accent
                    ? palette.accentZ
                    : palette.sideZ[y % palette.sideZ.length];

                drawFace(
                    [
                        project(x + 1, y + 1, z),
                        project(x + 1, y + 1, z + 1),
                        project(x + 1, y, z + 1),
                        project(x + 1, y, z),
                    ],
                    sideX
                );
                drawFace(
                    [
                        project(x, y + 1, z + 1),
                        project(x + 1, y + 1, z + 1),
                        project(x + 1, y, z + 1),
                        project(x, y, z + 1),
                    ],
                    sideZ
                );
                drawFace(
                    [
                        project(x, y + 1, z),
                        project(x + 1, y + 1, z),
                        project(x + 1, y + 1, z + 1),
                        project(x, y + 1, z + 1),
                    ],
                    top
                );
            });

            context.restore();

            if (yawReadout) {
                const degrees = Math.round(((yaw * 180) / Math.PI + 360) % 360);
                yawReadout.textContent = `${String(degrees).padStart(3, "0")}°`;
            }
        };

        const animate = (now) => {
            animation = 0;
            if (!visible) return;
            if (now - lastFrame > 32) {
                lastFrame = now;
                if (!dragging) {
                    const idle = reduceMotion.matches
                        ? 0
                        : Math.sin((now - idleStart) * 0.00023) * 0.055;
                    targetYaw = idleYaw + idle;
                }
                yaw += (targetYaw - yaw) * 0.09;
                draw();
            }
            animation = requestAnimationFrame(animate);
        };

        const start = () => {
            if (reduceMotion.matches) {
                draw();
                return;
            }
            if (!animation) animation = requestAnimationFrame(animate);
        };

        canvas.addEventListener("pointerdown", (event) => {
            if (event.button !== 0) return;
            dragging = true;
            lastX = event.clientX;
            canvas.setPointerCapture(event.pointerId);
            canvas.focus({ preventScroll: true });
            start();
        });

        canvas.addEventListener("pointermove", (event) => {
            if (!dragging) return;
            const delta = event.clientX - lastX;
            lastX = event.clientX;
            targetYaw = clamp(targetYaw + delta * 0.006, -0.7, 0.7);
            yaw = targetYaw;
            draw();
        });

        const stopDrag = (event) => {
            if (!dragging) return;
            dragging = false;
            idleYaw = yaw;
            idleStart = performance.now();
            if (canvas.hasPointerCapture(event.pointerId)) canvas.releasePointerCapture(event.pointerId);
            start();
        };

        canvas.addEventListener("pointerup", stopDrag);
        canvas.addEventListener("pointercancel", stopDrag);

        canvas.addEventListener("keydown", (event) => {
            if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
            event.preventDefault();
            event.stopPropagation();
            const delta = event.key === "ArrowRight" ? 0.08 : -0.08;
            targetYaw = clamp(yaw + delta, -0.7, 0.7);
            yaw = targetYaw;
            idleYaw = yaw;
            idleStart = performance.now();
            draw();
        });

        new ResizeObserver(() => {
            resize();
            draw();
        }).observe(stage);

        if ("IntersectionObserver" in window) {
            new IntersectionObserver((entries) => {
                visible = entries.some((entry) => entry.isIntersecting);
                if (visible) start();
            }).observe(stage);
        }

        window.addEventListener("valescent:theme", () => {
            palette = readPalette();
            draw();
        });

        document.fonts?.ready.then(() => {
            palette = readPalette();
            draw();
        });

        resize();
        palette = readPalette();
        draw();
        start();
    };

    const init = () => {
        initMenu();
        initSidebar();
        initVoxelPlate();
        initCodeLabels();
        initProgress();
        initThemeSync();

        sidebarReady((sidebar) => {
            initBreadcrumbs(sidebar);
            initToc();
            initPager(sidebar);
        });

        if (document.querySelector(".hero")) {
            document.body.classList.add("has-hero");
            document.title = "Valescent Engine · Documentation";
        }
    };

    if (document.readyState === "loading") {
        document.addEventListener("DOMContentLoaded", init);
    } else {
        init();
    }
})();
