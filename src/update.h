/* Update check against the GitHub releases of the project, through WinINet
 * (loaded at run time: plain Windows 95 does not have it until Internet
 * Explorer is installed). Windows 95/98/Me and early NT cannot negotiate the
 * TLS versions GitHub requires; the error then says so and names the page. */
#ifndef ZS_UPDATE_H
#define ZS_UPDATE_H
#include "base.h"

#define UPDATE_REPO "knivesbatz/zipshrink"
#define UPDATE_INSTALLER "ZipShrink-setup.exe"
/* The version updates are compared with; CI builds a copy that claims to
 * be older, to exercise a real download and install. */
#ifndef ZS_UPDATE_FROM
#define ZS_UPDATE_FROM ZS_VERSION
#endif
/* Automatic checks: at most once a week (seconds). */
#define UPDATE_INTERVAL (7 * 86400)
/* Only for releases without GitHub's per-asset SHA-256 digest; versions
 * 0.2 and 0.3 need it in every release. */
#define UPDATE_CHECKSUMS "SHA256SUMS"
/* Shipped inside the portable zip; its presence means "do not run setup". */
#define UPDATE_PORTABLE_MARKER "zipshrink-portable.txt"

typedef struct {
    char *version; /* "0.3.0" */
    char *tag, *page;
    char *installer_url, *sums_url;
    u8 sha[32]; /* the installer's SHA-256 from GitHub, when has_sha */
    int has_sha;
} Release;
void release_free(Release *r);
/* Reads GitHub's JSON for a release. */
int update_parse_release(const char *json, Release *r, char **err);

/* Asks GitHub for the latest release; *newer is 1 when it is newer. */
int update_check(Release *r, int *newer, char **err);
/* 1 if a.b.c `a` is newer than `b`. */
int version_newer(const char *a, const char *b);
/* 0 for a portable copy: updates are then downloaded by hand. */
int update_can_install(void);
typedef void (*DownloadCb)(void *ctx, u64 done, u64 total);
/* Downloads the installer to the temp folder and checks its SHA-256 (from
 * GitHub's asset digest, else the release's SHA256SUMS); *path receives
 * where it is. */
int update_download(const Release *r, char **path, DownloadCb cb, void *ctx, char **err);
/* Starts the installer silently: it waits for ZipShrink to close, upgrades
 * the installation in place and, with `restart`, starts ZipShrink again. */
int update_launch(const char *path, int restart, char **err);
void update_open_url(const char *url);
/* Finds `name`'s SHA-256 in a sha256sum listing. */
int update_expected_hash(const char *sums, const char *name, u8 out[32]);
#endif
