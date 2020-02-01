use super::run_server;
use crate::prelude::*;
use crate::test_h1_h2;
use crate::Error;

test_h1_h2! {
    fn query_params() -> Result<(), Error> {
        |bld: http::request::Builder| {
            let req = bld
                .uri("/path")
                .query("x", "y")
                .body(().into())?;
            let (server_req, client_res, _client_bytes) = run_server(req, "Ok")?;
            assert_eq!(client_res.status(), 200);
            assert_eq!(server_req.uri(), "/path?x=y");
            Ok(())
        }
    }

    fn query_params_doubled() -> Result<(), Error> {
        |bld: http::request::Builder| {
            let req = bld
                .uri("/path")
                .query("x", "y")
                .query("x", "y")
                .body(().into())?;
            let (server_req, client_res, _client_bytes) = run_server(req, "Ok")?;
            assert_eq!(client_res.status(), 200);
            assert_eq!(server_req.uri(), "/path?x=y&x=y");
            Ok(())
        }
    }

    fn request_header() -> Result<(), Error> {
        |bld: http::request::Builder| {
            let req = bld
                .uri("/path")
                .header("x-foo", "bar")
                .body(().into())?;
            let (server_req, client_res, _client_bytes) = run_server(req, "Ok")?;
            assert_eq!(client_res.status(), 200);
            assert_eq!(server_req.header("x-foo"), Some("bar"));
            Ok(())
        }
    }
}