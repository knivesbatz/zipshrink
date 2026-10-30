/* Update check against the GitHub releases of the project, through WinINet
 * (loaded at run time: plain Windows 95 does not have it until Internet
 * Explorer is installed). Windows 95/98/Me and early NT cannot negotiate the
 * TLS versions GitHub requires; the error then says so and names the page. */
#ifndef ZS_UPDATE_H
#define ZS_UPDATE_H
#include "base.h"

#define UPDATE_REPO "knivesbatz/zipshrink"
#define UPDATE_INSTALLER "ZipShrink-setup.exe"
#define UPDATE_CHECKSUMS "SHA256SUMS"
/* Shipped inside the portable zip; its presence means "do not run setup". */
#define UPDATE_PORTABLE_MARKER "zipshrink-portable.txt"

typedef struct {
    char *version; /* "0.3.0" */
    char *tag, *page;
    char *installer_url, *sums_url;
} Release;
void release_free(Release *r);

/* Asks GitHub for the latest release; *newer is 1 when it is newer. */
int update_check(Release *r, int *newer, char **err);
/* 1 if a.b.c `a` is newer than `b`. */
int version_newer(const char *a, const char *b);
/* 0 for a portable copy: updates are then downloaded by hand. */
int update_can_install(void);
typedef void (*DownloadCb)(void *ctx, u64 done, u64 total);
/* Downloads the installer to the temp folder and checks it against the
 * release's SHA256SUMS; *path receives where it is. */
int update_download(const Release *r, char **path, DownloadCb cb, void *ctx, char **err);
int update_launch(const char *path, char **err);
void update_open_url(const char *url);
/* Finds `name`'s SHA-256 in a sha256sum listing. */
int update_expected_hash(const char *sums, const char *name, u8 out[32]);
#endif
