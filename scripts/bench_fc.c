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

	if (!strcmp (op, "prepare")) {
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
