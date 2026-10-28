# icons

Artwork for the pycommonkit logo and README banner: a straight-down aerial
photograph of farmland, and stylized renderings of it. Square fields and
center-pivot circles share one road grid, as named segments share one store.

## Files

| File | What it is | License |
|---|---|---|
| `naip-finney-ks-2x.jpg` | Source photograph (NAIP orthoimagery) | Public domain |
| `commons-cartoon.jpg` | Rendering of the source, cartoon style | MIT |
| `commons-colored-pencil.jpg` | Rendering of the source, colored-pencil style | MIT |
| `commons-faded-illustration.jpg` | Rendering of the source, faded-illustration style (the logo) | MIT |
| `commons-monochrome-graphic.jpg` | Rendering of the source, monochrome-graphic style | MIT |
| `commons-oil-painting.jpg` | Rendering of the source, oil-painting style | MIT |
| `commons-oil-painting-2.jpg` | Rendering of the source, second oil-painting variant | MIT |
| `commons-pen-and-ink.jpg` | Rendering of the source, pen-and-ink style | MIT |
| `commons-tempera.jpg` | Rendering of the source, tempera style | MIT |

All are 1600 × 1296 JPEGs. The README banner, `docs/images/commons-banner.jpg`,
is a 1200 × 330 crop of `commons-faded-illustration.jpg` and is under the same
terms as it.

## Source photograph

`naip-finney-ks-2x.jpg` shows center-pivot irrigated farmland near Garden City,
Finney County, Kansas, about 6.4 × 5.2 km.

| Field | Value |
|---|---|
| Imagery | USDA Farm Service Agency, National Agriculture Imagery Program (NAIP) |
| Served by | USGS The National Map, `USGSNAIPImagery` ImageServer |
| Bounding box (WGS84) | −100.77648, 37.97658, −100.70352, 38.02342 |
| Projection | Web Mercator (EPSG:3857) |
| Retrieved | 2026-10-28 |

To reproduce it:

```
https://imagery.nationalmap.gov/arcgis/rest/services/USGSNAIPImagery/ImageServer/exportImage?bbox=-100.77648,37.97658,-100.70352,38.02342&bboxSR=4326&imageSR=3857&size=1600,1296&format=jpg&f=image
```

NAIP imagery is a work of the U.S. Government and is in the **public domain**;
the USGS service description calls it so. It may be used, modified and
redistributed for any purpose without permission or attribution. Attribution
is not required; as a courtesy, the service's credit line is:

> USGS, USDA, The National Map: Orthoimagery

The title, credit and rights statement are also embedded in the JPEG's XMP and
EXIF metadata.

## Renderings

The `commons-*.jpg` renderings are Copyright (c) 2026 Sean D. True and are
available under the **MIT License**, the same license as pycommonkit itself;
the full text is in [`../LICENSE`](../LICENSE). Under MIT, keep the copyright
notice and license text with substantial copies of a rendering.

The MIT License covers only what the renderings add. The underlying
photograph remains in the public domain, as above.
