/* Disk-space preflight: will the job fit, and why. */
#include "engine.h"
#include "miniz.h"

#define BATCH_SLACK (64ull << 20)

static u64 overhead(const Scan *s) {
    u64 names = 0;
    size_t i;
    for (i = 0; i < s->n; i++) names += strlen(s->e[i].name);
    return 2 * names + (u64)s->n * 230 + 200;
}

static u32 compress_sample(const u8 *data, size_t n, int level) {
    tdefl_compressor *c = zs_alloc(sizeof(tdefl_compressor));
    size_t out_cap = n + n / 2 + 1024, in_n = n, out_n;
    u8 *out = zs_alloc(out_cap);
    u32 r;
    tdefl_init(c, NULL, NULL, (int)tdefl_create_comp_flags_from_zip_params(level, -15, 0));
    out_n = out_cap;
    tdefl_compress(c, data, &in_n, out, &out_n, TDEFL_FINISH);
    r = (u32)((u64)out_n * 1000 / (n ? n : 1));
    zs_free(out);
    zs_free(c);
    return r > 1000 ? 1000 : r;
}

u32 sample_ratio(const Scan *s, int level) {
    size_t i, picks[24], np = 0;
    u64 weight = 0, weighted = 0;
    u8 *buf;
    if (level == 0) return 1000;
    /* The 12 largest files, plus 12 spread through the plan. */
    {
        size_t files = 0;
        for (i = 0; i < s->n; i++)
            if (!s->e[i].is_dir && s->e[i].size) files++;
        if (!files) return 1000;
        while (np < 12) {
            size_t best = (size_t)-1, k;
            for (i = 0; i < s->n; i++) {
                int taken = 0;
                if (s->e[i].is_dir || !s->e[i].size) continue;
                for (k = 0; k < np; k++)
                    if (picks[k] == i) taken = 1;
                if (!taken && (best == (size_t)-1 || s->e[i].size > s->e[best].size)) best = i;
            }
            if (best == (size_t)-1) break;
            picks[np++] = best;
        }
        {
            size_t step = files / 12 ? files / 12 : 1, seen = 0;
            for (i = 0; i < s->n && np < 24; i++) {
                if (s->e[i].is_dir || !s->e[i].size) continue;
                if (seen++ % step == 0) {
                    size_t k;
                    int taken = 0;
                    for (k = 0; k < np; k++)
                        if (picks[k] == i) taken = 1;
                    if (!taken) picks[np++] = i;
                }
            }
        }
    }
    buf = zs_alloc(512 << 10);
    for (i = 0; i < np; i++) {
        const PlanEntry *e = &s->e[picks[i]];
        HANDLE h = plat_open_read(e->src, 0);
        u32 got = 0;
        if (h == INVALID_HANDLE_VALUE) continue;
        plat_pread(h, buf, 512 << 10, 0, &got);
        CloseHandle(h);
        if (!got) continue;
        weight += e->size;
        weighted += (u64)compress_sample(buf, got, level) * e->size;
    }
    zs_free(buf);
    return weight ? (u32)(weighted / weight) : 1000;
}

static u64 est(u64 n, u32 r) { return n / 1000 * r + (n % 1000) * r / 1000; }

int preflight(const Scan *s, const JobPaths *paths, const Options *o, Preflight *p, char **err) {
    u64 same = 0, same_largest = 0, other = 0, ovh, usable;
    u32 dest_vol;
    size_t i;
    char *h1, *h2, *h3, *h4;
    memset(p, 0, sizeof(*p));
    p->dest_dir = zs_dirname(paths->output);
    if (!plat_free_space(p->dest_dir, &p->free_space)) return *err = zs_fmt("%s: %s", p->dest_dir, plat_error(GetLastError())), 0;
    p->min_free = o->min_free < MIN_FREE_FLOOR ? MIN_FREE_FLOOR : o->min_free;
    dest_vol = plat_volume_serial(p->dest_dir);
    for (i = 0; i < s->n; i++) {
        const PlanEntry *e = &s->e[i];
        if (e->is_dir) continue;
        if (dest_vol && plat_volume_serial(s->roots[e->root]) == dest_vol) {
            same += e->size;
            if (e->size > same_largest) same_largest = e->size;
        } else {
            other += e->size;
        }
    }
    p->files = s->files, p->dirs = s->dirs, p->bytes = s->bytes, p->largest = s->largest;
    p->same_volume_bytes = same;
    p->delete_originals = o->delete_originals;
    ovh = overhead(s);
    p->ratio_permille = sample_ratio(s, o->level);
    p->worst = zip_worst(s->bytes) + ovh;
    p->estimated = est(s->bytes, p->ratio_permille) + ovh;
    if (o->delete_originals) {
        u64 peak = zip_worst(same_largest), peak_est = est(same_largest, p->ratio_permille);
        u64 slack = BATCH_SLACK < zip_worst(same) ? BATCH_SLACK : zip_worst(same);
        u64 slack_est = BATCH_SLACK < est(same, p->ratio_permille) ? BATCH_SLACK : est(same, p->ratio_permille);
        if (slack > peak) peak = slack;
        if (slack_est > peak_est) peak_est = slack_est;
        p->need_worst = zip_worst(other) + peak + ovh;
        p->need_estimate = est(other, p->ratio_permille) + peak_est + ovh;
    } else {
        p->need_worst = p->worst;
        p->need_estimate = p->estimated;
    }
    usable = p->free_space > p->min_free ? p->free_space - p->min_free : 0;
    p->verdict = usable >= p->need_worst ? V_FITS : usable >= p->need_estimate ? V_PROBABLY : V_NO;
    for (i = 0; i < s->nwarnings; i++) pv_push(&p->warnings, zs_strdup(s->warnings[i]));
    p->fs_name = plat_fs_name(p->dest_dir);
    if (p->fs_name && zs_starts_with(p->fs_name, "FAT") && p->estimated >= 0xFFFFFFFFull)
        pv_push(&p->warnings, zs_strdup("the destination is a FAT drive, which cannot hold a file of 4 GB or more: split the archive into parts"));
    if (o->delete_originals && same == 0 && s->bytes)
        pv_push(&p->warnings, zs_strdup("the selected files are on a different drive from the archive, so deleting them does not free space on the destination"));
    h1 = zs_human(p->need_worst), h2 = zs_human(p->free_space), h3 = zs_human(p->min_free), h4 = zs_human(p->need_estimate);
    if (p->verdict == V_FITS) p->explanation = zs_fmt("It fits: the job needs at most %s of the %s available (keeping %s free).", h1, h2, h3);
    else if (p->verdict == V_PROBABLY)
        p->explanation = zs_fmt("It should fit: about %s is needed at the sampled compression ratio, but up to %s if the data does not compress; %s is available (keeping %s free). If space runs low the job pauses safely and can be resumed.", h4, h1, h2, h3);
    else
        p->explanation = zs_fmt("It does not fit: about %s is needed but only %s is available (keeping %s free).%s", h4, h2, h3,
                                o->delete_originals ? "" : " Deleting originals as they are archived, or another destination, would reduce this.");
    zs_free(h1), zs_free(h2), zs_free(h3), zs_free(h4);
    return 1;
}

void preflight_free(Preflight *p) {
    zs_free(p->dest_dir);
    zs_free(p->fs_name);
    zs_free(p->explanation);
    pv_free(&p->warnings, 1);
    memset(p, 0, sizeof(*p));
}
