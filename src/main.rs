extern crate hyper;
extern crate hyper_native_tls;
#[macro_use]
extern crate clap;
extern crate serde;
extern crate serde_json;
extern crate backblaze_b2;
extern crate sha1;

use hyper::Client;
use hyper::net::HttpsConnector;
use hyper_native_tls::NativeTlsClient;

use clap::{ArgMatches, App, Arg};

use backblaze_b2::B2Error;
use backblaze_b2::raw::authorize::{B2Credentials, B2Authorization};
use backblaze_b2::raw::download::{DownloadAuthorization};
use backblaze_b2::raw::upload::{UploadAuthorization};

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::fs::{File, metadata};
use std::sync::{RwLock, Arc};
use std::thread::{self, JoinHandle};

#[derive(Debug)]
enum Action {
    Upload(PathBuf, String),
    Download(String, PathBuf)
}

struct AuthSource {
    cred: B2Credentials,
    client: Client,
    bucket_name: String,
    bucket_id: String,
    auth: RwLock<B2Authorization>
}
impl AuthSource {
    fn re_auth(&self) {
        let mut lock = self.auth.write().unwrap();
        let auth = {
            let mut a = None;
            while let None = a {
                match self.cred.authorize(&self.client) {
                    Ok(b) => a = Some(b),
                    Err(e) => if e.should_back_off() {
                        a = None;
                    } else {
                        panic!(e);
                    }
                }
            }
            a.unwrap()
        };
        *lock = auth;
    }
    fn dl_auth(&self) -> DownloadAuthorization {
        let dl_auth = {
            let lock = self.auth.read().unwrap();
            lock.to_download_authorization()
        };
        dl_auth
    }
    fn up_auth(&self) -> UploadAuthorization {
        {
            let lock = self.auth.read().unwrap();
            loop {
                match lock.get_upload_url(&self.bucket_id, &self.client) {
                    Ok(url) => return url,
                    Err(e) => {
                        if e.should_obtain_new_authentication() {
                            break;
                        }
                        if e.should_back_off() { continue; }
                        panic!(e);
                    }
                }
            }
        }
        self.up_auth()
    }
    fn create(cred: B2Credentials, client: Client, bucket: String) -> AuthSource {
        let auth = {
            let mut a = None;
            while let None = a {
                match cred.authorize(&client) {
                    Ok(b) => a = Some(b),
                    Err(e) => if e.should_back_off() {
                        a = None;
                    } else {
                        panic!(e);
                    }
                }
            }
            a.unwrap()
        };
        let buckets = auth.list_buckets::<serde_json::value::Value>(&client).unwrap();
        let bucket = buckets.into_iter().filter(|b| b.bucket_name == bucket).next().unwrap();
        AuthSource {
            cred: cred,
            client: client,
            auth: RwLock::new(auth),
            bucket_name: bucket.bucket_name,
            bucket_id: bucket.bucket_id
        }
    }
}

fn main() {
    let m = cli_matches();
    let actions = get_actions(&m);
    if actions.len() == 0 { return; }
    let cred = get_credentials(&m);

    let ssl = NativeTlsClient::new().unwrap();
    let connector = HttpsConnector::new(ssl);
    let client = Client::with_connector(connector);

    let authsource = Arc::new(AuthSource::create(cred, client, m.value_of("bucket").unwrap().to_string()));
    let mut threads = Vec::new();
    for action in actions {
        threads.push(spawn_thread(action, authsource.clone()));
    }
    for t in threads {
        match t.join().unwrap() {
            Ok(msg) => println!("{}", msg),
            Err(e) => println!("fail: {:?}", e)
        }
    }
}

fn spawn_thread(action: Action, authsource: Arc<AuthSource>)
    -> JoinHandle<Result<String, B2Error>>
{
    match action {
        Action::Upload(l, b2) => thread::spawn(move || {
            let ssl = NativeTlsClient::new().unwrap();
            let connector = HttpsConnector::new(ssl);

            let file_size = metadata(l.as_path())?.len();

            let mut up = authsource.up_auth();
            let mut m = sha1::Sha1::new();
            let mut upload = match up.create_upload_file_request_sha1_at_end(
                b2.clone(), None, file_size, &connector
            ) {
                Ok(u) => u,
                Err(e) => {
                    if e.should_obtain_new_authentication() {
                        authsource.re_auth();
                        up = authsource.up_auth();
                    }
                    if e.should_obtain_new_authentication() || e.should_back_off() {
                        up.create_upload_file_request_sha1_at_end(
                            b2.clone(), None, file_size, &connector
                        )?
                    } else { return Err(e); }
                }
            };
            let mut file = File::open(l.as_path())?;
            let mut len = 1;
            let mut written = 0;
            println!("upload to {} has started", b2);
            while len > 0 {
                let mut buf = [0; 4096];
                len = file.read(&mut buf)?;
                upload.write_all(&buf[0..len])?;
                m.update(&buf[0..len]);
                written += len;
                println!("written {} bytes {}%", written, (written as f64) / (file_size as f64) * 100.0);
            }
            let digest = m.digest();
            match upload.finish::<serde_json::value::Value>(&format!("{}", digest)) {
                Ok(mfi) => Ok(format!("file {} uploaded, id: {}\ndownload url: {}/file/{}/{}",
                                      mfi.file_name, mfi.file_id, authsource.auth.read().unwrap().download_url,
                                      authsource.bucket_name, b2)),
                Err(e) => Err(e)
            }
        }),
        Action::Download(b2, l) => thread::spawn(move || {
            let down = authsource.dl_auth();
            let (mut resp, _) = down.download_file_by_name::<serde_json::value::Value>(
                &authsource.bucket_name, &b2, &authsource.client
            )?;
            let mut file = File::create(l.as_path())?;
            let mut len = 1;
            let mut written = 0;
            println!("download from {} has started", b2);
            while len > 0 {
                let mut buf = [0; 4096];
                len = resp.read(&mut buf)?;
                file.write_all(&buf[0..len])?;
                written += 0;
                println!("downloaded {} bytes", written);
            }
            Ok(format!("file {} downloaded", b2))
        }),
    }
}

fn get_actions(matches: &ArgMatches) -> Vec<Action> {
    let mut vec = Vec::new();
    if let Some(mut iter) = matches.values_of_os("upload") {
        while let Some(local) = iter.next() {
            let dest = iter.next().unwrap().to_os_string().into_string().unwrap();
            vec.push(Action::Upload(Path::new(local).to_path_buf(), dest));
        }
    }
    if let Some(mut iter) = matches.values_of_os("download") {
        while let Some(b2) = iter.next() {
            let dest = iter.next().unwrap();
            vec.push(Action::Download(b2.to_os_string().into_string().unwrap(), Path::new(dest).to_path_buf()));
        }
    }
    vec
}
fn get_credentials(matches: &ArgMatches) -> B2Credentials {
    match serde_json::from_reader(File::open(matches.value_of_os("auth").unwrap()).unwrap()) {
        Ok(cred) => cred,
        Err(e) => panic!("unable to fetch credentials: {}", e)
    }
}

fn cli_matches() -> ArgMatches<'static> {
    let auth_arg = Arg::with_name("auth")
        .short("a")
        .long("auth")
        .takes_value(true)
        .value_name("FILE")
        .allow_hyphen_values(true)
        .default_value("credentials.txt")
        .help("Specifies the json file containing the credentials for b2");
    let bucket_arg = Arg::with_name("bucket")
        .short("b")
        .long("bucket")
        .takes_value(true)
        .value_name("BUCKET")
        .allow_hyphen_values(true)
        .required(true)
        .help("Specify the b2 bucket to interact with");
    let upload_actions = Arg::with_name("upload")
        .short("u")
        .long("upload")
        .number_of_values(2)
        .multiple(true)
        .value_names(&["LOCAL", "DESTINATION"])
        .allow_hyphen_values(true)
        .help("Each occurence of this action specifies a file to upload");
    let download_actions = Arg::with_name("download")
        .short("d")
        .long("download")
        .number_of_values(2)
        .multiple(true)
        .value_names(&["B2FILE", "DESTINATION"])
        .allow_hyphen_values(true)
        .help("Each occurence of this action specifies a file to download");
    App::new("b2cli")
        .author("Alice Ryhl <alice@ryhl.io>")
        .version(crate_version!())
        .about("Allows interacting with backblaze b2 from the command line.")
        .arg(auth_arg)
        .arg(bucket_arg)
        .arg(upload_actions)
        .arg(download_actions)
        .get_matches()
}
