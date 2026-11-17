#include <CoreGraphics/CoreGraphics.h>
#include <stdio.h>
#include <string.h>

int main(void) {
    float source[] = {0,.25,.5,1, 2,2,2,1, .125,.0625,0,.5, 12.5,12.5,12.5,1};
    CGColorSpaceRef space = CGColorSpaceCreateWithName(kCGColorSpaceExtendedLinearSRGB);
    CGDataProviderRef provider = CGDataProviderCreateWithData(NULL, source, sizeof(source), NULL);
    CGImageRef image = CGImageCreate(2, 2, 32, 128, 32, space,
        kCGBitmapByteOrder32Little | kCGBitmapFloatComponents | kCGImageAlphaPremultipliedLast,
        provider, NULL, false, kCGRenderingIntentDefault);
    if (!space || !provider || !image) return 2;
    printf("SDK None=%u source headroom=%g\n", (unsigned)kCGToneMappingNone, CGImageGetContentHeadroom(image));
    const float headrooms[] = {-1, 0, 1, 16};
    for (unsigned h=0; h<4; h++) {
        for (unsigned method=0; method<=6; method++) {
            float output[16] = {0};
            CGContextRef context = CGBitmapContextCreate(output, 2, 2, 32, 32, space,
                kCGBitmapByteOrder32Little | kCGBitmapFloatComponents | kCGImageAlphaPremultipliedLast);
            if (!context) return 3;
            bool set = headrooms[h]<0 ? false : CGContextSetEDRTargetHeadroom(context, headrooms[h]);
            bool drawn = false;
            if (method==6) CGContextDrawImage(context, CGRectMake(0,0,2,2), image);
            else drawn = CGContextDrawImageApplyingToneMapping(context, CGRectMake(0,0,2,2), image, (CGToneMapping)method, NULL);
            CGContextRelease(context);
            printf("headroom=%g set=%u method=%u result=%u values=", headrooms[h],set,method,drawn);
            for (unsigned n=0; n<16; n++) printf(" %.9g",output[n]);
            puts("");
        }
    }
    CGImageRelease(image); CGDataProviderRelease(provider); CGColorSpaceRelease(space);
    return 0;
}
