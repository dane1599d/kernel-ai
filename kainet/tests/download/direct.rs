//! `Direct`: a release asset's URL, its redirect, and which URL each range is asked of.

use super::*;

/// A release asset's URL and where its redirect leads, as GitHub's go.
const RELEASE: &str = "https://releases.kai.test/o/r/releases/download/t/blob";
const ASSET: &str = "https://assets.kai.test/assets/t/blob?sig=1";

fn url(text: &str) -> Url {
    Url::parse(text).unwrap()
}

/// A server for [`Direct`]: the release URL redirects to the asset, which
/// answers each range in turn as `asset` says, a status or an error. It
/// records each URL asked.
fn asked_of<'a>(
    asked: &'a RefCell<Vec<String>>,
    asset: &'a RefCell<Vec<Result<u16, HttpError>>>,
) -> Box<Get<'a>> {
    Box::new(
        move |at: &Url, _: Range, sink: &mut dyn FnMut(&[u8]) -> Result<(), HttpError>| {
            let text = format!("https://{}{}", at.host, at.path);
            asked.borrow_mut().push(text.clone());
            let status = if text == RELEASE {
                206
            } else {
                asset.borrow_mut().remove(0)?
            };
            if status == 206 {
                sink(&[7; 4])?;
            }
            let answer = Answer {
                status,
                retry_after: None,
            };
            Ok((answer, url(ASSET)))
        },
    )
}

const RANGE: Range = Range {
    first: 0,
    last: Some(3),
};

fn take(direct: &mut Direct, get: &mut Get) -> Result<(u16, usize), HttpError> {
    let mut got = 0;
    let answer = direct.range(
        RANGE,
        &mut |b| {
            got += b.len();
            Ok(())
        },
        get,
    )?;
    Ok((answer.status, got))
}

/// After the first range, every range skips the redirect and its connection.
#[test]
fn direct_asks_the_url_that_answered_after_the_first_range() {
    let asked = RefCell::new(Vec::new());
    let asset = RefCell::new(vec![Ok(206), Ok(206)]);
    let mut get = asked_of(&asked, &asset);
    let mut direct = Direct::new(url(RELEASE));
    for _ in 0..3 {
        assert_eq!(take(&mut direct, &mut get), Ok((206, 4)));
    }
    let asset_path = "https://assets.kai.test/assets/t/blob?sig=1";
    assert_eq!(*asked.borrow(), [RELEASE, asset_path, asset_path]);
    assert_eq!(direct.url().host, "assets.kai.test");
}

/// A signed asset URL that has expired answers 403: that range is asked of
/// the release URL again, once, and its new asset URL is kept.
#[test]
fn an_expired_asset_url_is_asked_of_the_target_again_once() {
    let asked = RefCell::new(Vec::new());
    let asset = RefCell::new(vec![Ok(403), Ok(206)]);
    let mut get = asked_of(&asked, &asset);
    let mut direct = Direct::new(url(RELEASE));
    assert_eq!(take(&mut direct, &mut get), Ok((206, 4)));
    assert_eq!(take(&mut direct, &mut get), Ok((206, 4)));
    assert_eq!(take(&mut direct, &mut get), Ok((206, 4)));
    let hosts: Vec<bool> = asked.borrow().iter().map(|a| a == RELEASE).collect();
    assert_eq!(hosts, [true, false, true, false]);
}

/// An error, a cut say, forgets the asset URL: the next range, a resume's
/// too, goes by the release URL.
#[test]
fn an_error_sends_the_next_range_by_the_target() {
    let asked = RefCell::new(Vec::new());
    let asset = RefCell::new(vec![Err(HttpError::Truncated)]);
    let mut get = asked_of(&asked, &asset);
    let mut direct = Direct::new(url(RELEASE));
    assert_eq!(take(&mut direct, &mut get), Ok((206, 4)));
    assert_eq!(take(&mut direct, &mut get), Err(HttpError::Truncated));
    assert_eq!(direct.url().host, "releases.kai.test");
    assert_eq!(take(&mut direct, &mut get), Ok((206, 4)));
    let hosts: Vec<bool> = asked.borrow().iter().map(|a| a == RELEASE).collect();
    assert_eq!(hosts, [true, false, true]);
}

/// A status that is not 206 from the release URL itself comes back as it
/// is, asked once, and nothing is kept.
#[test]
fn a_target_that_does_not_answer_206_is_asked_once() {
    let asked = RefCell::new(Vec::new());
    let mut get = |at: &Url, _: Range, _: &mut dyn FnMut(&[u8]) -> Result<(), HttpError>| {
        asked.borrow_mut().push(at.host.clone());
        let answer = Answer {
            status: 404,
            retry_after: None,
        };
        Ok((answer, at.clone()))
    };
    let mut direct = Direct::new(url(RELEASE));
    assert_eq!(take(&mut direct, &mut get), Ok((404, 0)));
    assert_eq!(asked.borrow().len(), 1);
    assert_eq!(direct.url().host, "releases.kai.test");
}

/// A whole download through [`Direct`]: one request by the release URL, the
/// rest straight to the asset, and the target checks out.
#[test]
fn a_download_through_direct_redirects_once() {
    let target = blob(4 * MIN_CHUNK as usize);
    let list = Chunks::of(&target, MIN_CHUNK);
    let asked = RefCell::new(Vec::new());
    let mut get = |at: &Url, range: Range, sink: &mut dyn FnMut(&[u8]) -> Result<(), HttpError>| {
        asked.borrow_mut().push(at.host.clone());
        let end = range.last.map_or(target.len(), |last| last as usize + 1);
        sink(&target[range.first as usize..end])?;
        Ok((Answer::PARTIAL, url(ASSET)))
    };
    let mut direct = Direct::new(url(RELEASE));
    let mut partial = Memory::default();
    let report = download(&list, &mut partial, &mut |range, sink| {
        direct.range(range, sink, &mut get)
    });
    assert_eq!(
        report,
        Ok(Report {
            kept: 0,
            fetched: 4
        })
    );
    assert_eq!(
        *asked.borrow(),
        [
            "releases.kai.test",
            "assets.kai.test",
            "assets.kai.test",
            "assets.kai.test"
        ]
    );
}

/// An asset URL that passes bytes on and then answers other than 206 is not
/// asked again: those bytes are already the caller's, and `download` refuses
/// the status.
#[test]
fn bytes_passed_on_before_a_status_are_not_asked_for_again() {
    let asked = RefCell::new(Vec::new());
    let mut get = |at: &Url, _: Range, sink: &mut dyn FnMut(&[u8]) -> Result<(), HttpError>| {
        asked.borrow_mut().push(at.host.clone());
        sink(&[7; 4])?;
        let status = if asked.borrow().len() == 2 { 200 } else { 206 };
        let answer = Answer {
            status,
            retry_after: None,
        };
        Ok((answer, url(ASSET)))
    };
    let mut direct = Direct::new(url(RELEASE));
    assert_eq!(take(&mut direct, &mut get), Ok((206, 4)));
    assert_eq!(take(&mut direct, &mut get), Ok((200, 4)));
    assert_eq!(direct.url().host, "releases.kai.test");
    assert_eq!(*asked.borrow(), ["releases.kai.test", "assets.kai.test"]);
}
