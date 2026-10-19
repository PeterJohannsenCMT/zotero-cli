use anyhow::{Context, Result};

pub struct HttpClient {
    agent: ureq::Agent,
    default_headers: Vec<(String, String)>,
}

impl HttpClient {
    pub fn new(user_agent: &str, default_headers: Vec<(String, String)>) -> Self {
        let agent = ureq::AgentBuilder::new()
            .user_agent(user_agent)
            .timeout(std::time::Duration::from_secs(10))
            .build();
        Self {
            agent,
            default_headers,
        }
    }

    pub fn get(&self, url: &str) -> ureq::Request {
        self.request("GET", url)
    }

    pub fn post(&self, url: &str) -> ureq::Request {
        self.request("POST", url)
    }

    pub fn patch(&self, url: &str) -> ureq::Request {
        self.request("PATCH", url)
    }

    pub fn delete(&self, url: &str) -> ureq::Request {
        self.request("DELETE", url)
    }

    fn request(&self, method: &str, url: &str) -> ureq::Request {
        let mut request = self.agent.request(method, url);
        for (name, value) in &self.default_headers {
            request = request.set(name, value);
        }
        request
    }
}

pub fn response(
    result: std::result::Result<ureq::Response, ureq::Error>,
) -> Result<ureq::Response> {
    match result {
        Ok(resp) => Ok(resp),
        Err(ureq::Error::Status(_, resp)) => Ok(resp),
        Err(err) => Err(err).context("request failed"),
    }
}

pub fn is_success(status: u16) -> bool {
    (200..300).contains(&status)
}
