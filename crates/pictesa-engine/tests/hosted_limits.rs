//! Limits that keep one hosted account from harming the server or learning about other accounts.
use pictesa_engine::{boards::BoardRole, Config, Engine, Mode};

async fn hosted() -> Engine {
    // This test binary alone sets the quota.
    std::env::set_var("PICTESA_USER_STORAGE_MB", "1");
    let mut cfg = Config::local(std::env::temp_dir().join(format!("pictesa-hosted-limits-{}", pictesa_engine::new_id())));
    cfg.mode = Mode::Hosted;
    Engine::open(cfg).await.unwrap()
}

#[tokio::test]
async fn hosted_storage_has_a_limit_per_account() {
    let e = hosted().await;
    let user = e.signup("full@example.com", "correct horse battery", None).await.unwrap();
    // Noise does not compress, so the PNG stays well above 1 MB.
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut noise = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed as u8
    };
    let img = image::RgbImage::from_fn(900, 900, |_, _| image::Rgb([noise(), noise(), noise()]));
    let mut png = std::io::Cursor::new(Vec::new());
    img.write_to(&mut png, image::ImageFormat::Png).unwrap();
    let png = png.into_inner();
    assert!(png.len() > 1 << 20, "the test image must exceed the 1 MB quota");
    let err = e.upload(&user.id, &png, Some("big.png"), None).await.unwrap_err();
    assert!(err.to_string().contains("storage is full"), "{err}");
}

#[tokio::test]
async fn guessing_emails_through_board_sharing_is_slowed() {
    let e = hosted().await;
    let owner = e.signup("owner@example.com", "correct horse battery", None).await.unwrap();
    let board = e.create_board(&owner.id, "Shared").await.unwrap();
    let mut last = String::new();
    for i in 0..11 {
        last = e.share_board(&owner.id, &board.id, &format!("nobody{i}@example.com"), BoardRole::Viewer).await.unwrap_err().to_string();
    }
    assert!(last.contains("Too many"), "{last}");
}
