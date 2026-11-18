/* Private generated-metadata oracle. Uses the actual installed SPA filter. */
#include <stdio.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <spa/pod/filter.h>
#include <spa/pod/iter.h>

static struct spa_pod *load(const char *path, uint64_t *words, size_t capacity) {
    FILE *f = fopen(path, "rb");
    if (!f) return NULL;
    size_t size = fread(words, 1, capacity, f);
    int extra = fgetc(f);
    fclose(f);
    if (extra != EOF || size < sizeof(struct spa_pod)) return NULL;
    struct spa_pod *pod = (struct spa_pod *)words;
    if (SPA_POD_SIZE(pod) > size || !spa_pod_is_object(pod)) return NULL;
    return pod;
}
int main(int argc, char **argv) {
    if (argc != 4) return 2;
    uint64_t a[8192], b[8192], output[8192];
    struct spa_pod *source = load(argv[1], a, sizeof(a));
    struct spa_pod *offer = load(argv[2], b, sizeof(b));
    if (!source || !offer) return 2;
    struct spa_pod_builder builder = SPA_POD_BUILDER_INIT(output, sizeof(output));
    struct spa_pod *result = NULL;
    int reverse = strcmp(argv[3], "reverse") == 0;
    int status = spa_pod_filter(&builder, &result, reverse ? offer : source, reverse ? source : offer);
    if (status < 0) return 10;
    if (!result || SPA_POD_SIZE(result) > sizeof(output)) return 2;
    spa_pod_object_fixate((struct spa_pod_object *)result);
    return fwrite(result, 1, SPA_POD_SIZE(result), stdout) == SPA_POD_SIZE(result) ? 0 : 2;
}
