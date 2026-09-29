`airdrop-odc.cpio` is an uncompressed POSIX odc archive generated with macOS
bsdtar (`--format=odc --uid 0 --gid 0`). It contains the directory `./` and
`./IMG_9457.jpg`, whose synthetic contents are
`ff d8 ff e0` + ASCII `whoosh-fixture` + `ff d9`.

The fixture checks compatibility with an independent archive writer rather
than only round-tripping Whoosh's own newc encoder. It contains no user photo.
