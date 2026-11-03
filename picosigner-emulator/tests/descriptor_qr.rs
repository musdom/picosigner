use embedded_graphics::{geometry::Size, pixelcolor::Rgb565};
use embedded_graphics_simulator::SimulatorDisplay;
use picosigner::ui::draw_descriptor_qr;
use picosigner_qr::DescriptorQrError;
use picosigner_wallet::{AccountType, Network, WalletSession};

#[test]
fn every_wallet_type_exports_a_decodable_descriptor_on_every_network() {
    for network in [Network::Mainnet, Network::Signet, Network::Testnet4] {
        let mut wallets = Vec::new();
        for account in [
            AccountType::NestedSegwit,
            AccountType::NativeSegwit,
            AccountType::Taproot,
        ] {
            wallets.push(WalletSession::new_with_options(
                [0u8; 16], network, account, "",
            ));
        }
        wallets.push(WalletSession::new_frost([1u8; 32], network));
        for wallet in wallets {
            let export = wallet.export().unwrap();
            let mut display = SimulatorDisplay::<Rgb565>::new(Size::new(240, 320));
            draw_descriptor_qr(&mut display, export.descriptor.as_bytes()).unwrap();
            let pixels = display.to_be_bytes();
            let mut decoder = quircs::Quirc::new();
            let code = decoder
                .identify_with_luma(240, 320, |index| {
                    if pixels[index * 2] == 0 && pixels[index * 2 + 1] == 0 {
                        0
                    } else {
                        255
                    }
                })
                .next()
                .expect("descriptor QR must be visible")
                .unwrap();
            assert_eq!(code.decode().unwrap().payload, export.descriptor.as_bytes());
        }
    }
}

#[test]
fn descriptor_qr_reports_capacity_errors() {
    let mut display = SimulatorDisplay::<Rgb565>::new(Size::new(240, 320));
    assert!(draw_descriptor_qr(&mut display, &[b'x'; 192]).is_ok());
    assert_eq!(
        draw_descriptor_qr(&mut display, &[b'x'; 193]),
        Err(DescriptorQrError::TooLarge)
    );
}
