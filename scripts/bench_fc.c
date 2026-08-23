/* Time libfontconfig doing the same operations as examples/bench.rs.
 *
 * Built and driven by scripts/bench.sh. The output format is identical to the
 * Rust side -- `<op> <iterations> <nanoseconds> <checksum>` -- so the two can
 * be compared directly.
 *
 * The pairing is deliberate about a few things:
 *
 *  - `load` runs once per process, because fontconfig keeps every cache it
 *    has read in a process-wide table (FcCacheInsert). Looping it in-process
 *    would time its memoisation against our real work.
 *
 *  - `list` touches three strings per font rather than counting, so that the
 *    cost of resolving them is in both numbers.
 *
 *  - `match` and `sort` do the substitution both libraries require before
 *    matching, and time it, because a caller cannot skip it.
 *
 *  - The checksum stops the compiler deleting the work, and catches a
 *    benchmark that has quietly stopped doing any.
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#include <fontconfig/fontconfig.h>

static const char *QUERIES[] = {
	"DejaVu Sans",
	"sans-serif",
	"serif:weight=200",
	"monospace",
	"NoSuchFamilyAnywhere",
	":lang=ja",
	":lang=en:weight=200:slant=100",
	"Noto Sans:lang=ar",
};
static const int NQUERIES = sizeof (QUERIES) / sizeof (QUERIES[0]);

/* A script, as a language tag and eight characters sampled from it: the
 * shape of query a fallback picker asks. See examples/bench.rs, which has
 * the same table. */
struct Script {
	const char *lang;
	FcChar32    chars[8];
};
static const struct Script SCRIPTS[] = {
	{ "en",	{ 0x41, 0x61, 0x7a, 0xe9, 0xf1, 0xfc, 0xdf, 0x152 } },
	{ "el",	{ 0x3b1, 0x3b2, 0x3b3, 0x3b4, 0x3b5, 0x3b6, 0x3b7, 0x3b8 } },
	{ "ru",	{ 0x430, 0x431, 0x432, 0x433, 0x434, 0x435, 0x436, 0x437 } },
	{ "he",	{ 0x5d0, 0x5d1, 0x5d2, 0x5d3, 0x5d4, 0x5d5, 0x5d6, 0x5d7 } },
	{ "ar",	{ 0x627, 0x628, 0x629, 0x62a, 0x62b, 0x62c, 0x62d, 0x62e } },
	{ "hi",	{ 0x905, 0x906, 0x907, 0x908, 0x909, 0x90a, 0x90b, 0x90c } },
	{ "zh-cn", { 0x4e00, 0x4e01, 0x4e02, 0x4e03, 0x4e04, 0x4e05, 0x4e06, 0x4e07 } },
	{ "ja",	{ 0x3042, 0x3044, 0x3046, 0x3048, 0x304a, 0x304b, 0x304d, 0x304f } },
	{ "ko",	{ 0xac00, 0xac01, 0xac02, 0xac03, 0xac04, 0xac05, 0xac06, 0xac07 } },
	{ "th",	{ 0xe01, 0xe02, 0xe03, 0xe04, 0xe05, 0xe06, 0xe07, 0xe08 } },
};
static const int NSCRIPTS = sizeof (SCRIPTS) / sizeof (SCRIPTS[0]);

/* The fallback query: a charset and a language, built rather than parsed so
 * that both sides are certainly asking the same thing. */
static FcPattern *
fallback (FcConfig *config, const struct Script *script)
{
	FcPattern *p = FcPatternCreate ();
	FcCharSet *cs = FcCharSetCreate ();
	int        i;

	for (i = 0; i < 8; i++)
		FcCharSetAddChar (cs, script->chars[i]);
	FcPatternAddCharSet (p, FC_CHARSET, cs);
	FcCharSetDestroy (cs);
	FcPatternAddString (p, FC_LANG, (const FcChar8 *)script->lang);

	FcConfigSubstitute (config, p, FcMatchPattern);
	FcDefaultSubstitute (p);
	return p;
}

static unsigned long long
now_ns (void)
{
	struct timespec ts;
	clock_gettime (CLOCK_MONOTONIC, &ts);
	return (unsigned long long)ts.tv_sec * 1000000000ull + (unsigned long long)ts.tv_nsec;
}

/* The length of one string property, or 0, mirroring the Rust side. */
static unsigned long long
string_len (FcPattern *p, const char *object)
{
	FcChar8 *s = NULL;
	if (FcPatternGetString (p, object, 0, &s) != FcResultMatch || !s)
		return 0;
	return (unsigned long long)strlen ((const char *)s);
}

/* A query with configuration and defaults applied, as matching expects. */
static FcPattern *
prepared (FcConfig *config, const char *name)
{
	FcPattern *p = FcNameParse ((const FcChar8 *)name);
	if (!p)
		return NULL;
	FcConfigSubstitute (config, p, FcMatchPattern);
	FcDefaultSubstitute (p);
	return p;
}

int
main (int argc, char **argv)
{
	const char *op = argc > 1 ? argv[1] : "noop";
	long iterations = argc > 2 ? atol (argv[2]) : 1;
	unsigned long long checksum = 0;
	unsigned long long start, elapsed;

	if (!strcmp (op, "noop")) {
		/* Process start and dynamic linking, and nothing else. Nothing
		 * of fontconfig is touched, so the loader still resolves it. */
		start = now_ns ();
		elapsed = now_ns () - start;
		printf ("%s %ld %llu %llu\n", op, iterations, elapsed, checksum);
		return 0;
	}

	if (!strcmp (op, "config")) {
		start = now_ns ();
		FcConfig *config = FcInitLoadConfig ();
		elapsed = now_ns () - start;
		if (!config) {
			fprintf (stderr, "FcInitLoadConfig failed\n");
			return 1;
		}
		FcStrList *files = FcConfigGetConfigFiles (config);
		FcChar8 *f;
		while ((f = FcStrListNext (files)))
			checksum++;
		FcStrListDone (files);
		printf ("%s %ld %llu %llu\n", op, iterations, elapsed, checksum);
		return 0;
	}

	if (!strcmp (op, "load")) {
		/* Configuration, then every cache. FcInitLoadConfigAndFonts is
		 * the call a program makes before it can answer anything. */
		start = now_ns ();
		FcConfig *config = FcInitLoadConfigAndFonts ();
		FcFontSet *set = FcConfigGetFonts (config, FcSetSystem);
		checksum = set ? (unsigned long long)set->nfont : 0;
		elapsed = now_ns () - start;
		printf ("%s %ld %llu %llu\n", op, iterations, elapsed, checksum);
		return 0;
	}

	if (!strcmp (op, "info")) {
		FcConfig  *config = FcInitLoadConfigAndFonts ();
		FcFontSet *sys = FcConfigGetFonts (config, FcSetSystem);
		FcFontSet *app = FcConfigGetFonts (config, FcSetApplication);
		printf ("system=%d application=%d\n", sys ? sys->nfont : -1, app ? app->nfont : -1);
		return 0;
	}

	/* Everything below loads once and then loops, which is what both
	 * libraries do in a running program. The load is not timed. */
	FcConfig *config = FcInitLoadConfigAndFonts ();
	if (!config) {
		fprintf (stderr, "FcInitLoadConfigAndFonts failed\n");
		return 1;
	}

	if (!strcmp (op, "charmatch") || !strcmp (op, "charsort")) {
		int sorting = !strcmp (op, "charsort");
		start = now_ns ();
		for (long i = 0; i < iterations; i++) {
			FcPattern *p = fallback (config, &SCRIPTS[i % NSCRIPTS]);
			FcResult   result;
			if (sorting) {
				FcFontSet *fs = FcFontSort (config, p, FcTrue, NULL, &result);
				if (fs) {
					checksum += (unsigned long long)fs->nfont;
					FcFontSetSortDestroy (fs);
				}
			} else {
				FcPattern *found = FcFontMatch (config, p, &result);
				if (found) {
					checksum += string_len (found, FC_FILE);
					FcPatternDestroy (found);
				}
			}
			FcPatternDestroy (p);
		}
		elapsed = now_ns () - start;
	} else if (!strcmp (op, "prepare")) {
		/* Parsing, substitution and defaults: what both libraries do
		 * before a match can start. */
		start = now_ns ();
		for (long i = 0; i < iterations; i++) {
			FcPattern *p = prepared (config, QUERIES[i % NQUERIES]);
			checksum += string_len (p, FC_FAMILY);
			FcPatternDestroy (p);
		}
		elapsed = now_ns () - start;
	} else if (!strcmp (op, "list")) {
		start = now_ns ();
		for (long i = 0; i < iterations; i++) {
			FcPattern   *pat = FcPatternCreate ();
			FcObjectSet *os = FcObjectSetBuild (FC_FAMILY, FC_FILE, FC_STYLE, (char *)0);
			FcFontSet   *fs = FcFontList (config, pat, os);
			for (int f = 0; fs && f < fs->nfont; f++) {
				checksum += string_len (fs->fonts[f], FC_FAMILY);
				checksum += string_len (fs->fonts[f], FC_FILE);
				checksum += string_len (fs->fonts[f], FC_STYLE);
			}
			if (fs)
				FcFontSetDestroy (fs);
			FcObjectSetDestroy (os);
			FcPatternDestroy (pat);
		}
		elapsed = now_ns () - start;
	} else if (!strcmp (op, "match")) {
		start = now_ns ();
		for (long i = 0; i < iterations; i++) {
			FcPattern *p = prepared (config, QUERIES[i % NQUERIES]);
			FcResult   result;
			FcPattern *found = FcFontMatch (config, p, &result);
			if (found) {
				checksum += string_len (found, FC_FILE);
				FcPatternDestroy (found);
			}
			FcPatternDestroy (p);
		}
		elapsed = now_ns () - start;
	} else if (!strcmp (op, "sort")) {
		start = now_ns ();
		for (long i = 0; i < iterations; i++) {
			FcPattern *p = prepared (config, QUERIES[i % NQUERIES]);
			FcResult   result;
			FcFontSet *fs = FcFontSort (config, p, FcTrue, NULL, &result);
			if (fs) {
				checksum += (unsigned long long)fs->nfont;
				FcFontSetSortDestroy (fs);
			}
			FcPatternDestroy (p);
		}
		elapsed = now_ns () - start;
	} else {
		fprintf (stderr, "unknown operation %s\n", op);
		return 1;
	}

	printf ("%s %ld %llu %llu\n", op, iterations, elapsed, checksum);
	return 0;
}
