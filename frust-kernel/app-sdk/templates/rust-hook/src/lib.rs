wit_bindgen::generate!({ path: "wit", world: "plugin" });

use exports::frust::plugin::hooks::{Entry, Guest};

struct App;

impl Guest for App {
    fn validate(doc: Vec<Entry>) -> Result<Vec<Entry>, String> {
        Ok(doc)
    }

    fn spin() {}

    fn hog() {}
}

export!(App);
