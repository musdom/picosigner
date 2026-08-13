use quircs::{DataType, EccLevel, Eci, Quirc};

#[test]
fn detects_and_decodes_upstream_fixture() {
    let image = image::open("tests/data/Hello+World.png")
        .unwrap()
        .into_luma8();
    let mut decoder = Quirc::default();
    let codes: Vec<_> = decoder
        .identify(image.width() as usize, image.height() as usize, &image)
        .collect::<Result<_, _>>()
        .unwrap();

    assert_eq!(codes.len(), 2);

    let hello = codes[0].decode().unwrap();
    assert_eq!(hello.version, 1);
    assert_eq!(hello.ecc_level, EccLevel::H);
    assert_eq!(hello.data_type, Some(DataType::Byte));
    assert_eq!(hello.eci, Some(Eci::Utf8));
    assert_eq!(hello.payload, b"Hello");

    let world = codes[1].decode().unwrap();
    assert_eq!(world.version, 1);
    assert_eq!(world.ecc_level, EccLevel::H);
    assert_eq!(world.data_type, Some(DataType::Byte));
    assert_eq!(world.eci, Some(Eci::Utf8));
    assert_eq!(world.payload, b"World");
}

#[test]
fn flipping_twice_preserves_a_code() {
    let image = image::open("tests/data/Hello+World.png")
        .unwrap()
        .into_luma8();
    let mut decoder = Quirc::default();
    let mut code = decoder
        .identify(image.width() as usize, image.height() as usize, &image)
        .next()
        .unwrap()
        .unwrap();
    let original = code.cell_bitmap;
    code.flip();
    code.flip();
    assert_eq!(code.cell_bitmap, original);
    assert_eq!(code.decode().unwrap().payload, b"Hello");
}
