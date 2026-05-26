use std::borrow::Cow;

/// A struct representing a page view within the telemetry system.
///
/// This struct contains information about a page view, including the path and an optional title.
/// It is used by the [`Session::record_new_page`] method to describe a page which is being viewed
/// within the application.
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct Page {
    pub(crate) url: Cow<'static, str>,
    pub(crate) title: Option<Cow<'static, str>>,
}

impl Page {
    /// Creates a new [`Page`] with the provided URL.
    pub fn new(url: impl Into<Cow<'static, str>>) -> Self {
        Self {
            url: url.into(),
            title: None,
        }
    }

    /// Adds a title to the page view, which may be used by the telemetry system to provide additional context about the page.
    ///
    /// ## Example
    /// ```
    /// use tracing_batteries::Page;
    ///
    /// let page = Page::new("/home").with_title("Home Page");
    /// ```
    pub fn with_title(mut self, title: impl Into<Cow<'static, str>>) -> Self {
        self.title = Some(title.into());
        self
    }
}

impl Default for Page {
    fn default() -> Self {
        Self {
            url: Cow::Borrowed("/"),
            title: None,
        }
    }
}

impl From<Cow<'static, str>> for Page {
    fn from(url: Cow<'static, str>) -> Self {
        Self::new(url)
    }
}

impl From<&'static str> for Page {
    fn from(url: &'static str) -> Self {
        Self::new(url)
    }
}

impl From<String> for Page {
    fn from(url: String) -> Self {
        Self::new(url)
    }
}
