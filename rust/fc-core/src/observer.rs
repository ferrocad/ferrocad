//! Observer callbacks fired by the document on changes.

use crate::document::ObjectId;
use crate::property::Property;

pub trait Observer {
    fn on_object_added(&mut self, object: ObjectId);
    fn on_property_changed(&mut self, object: ObjectId, name: &str, old: Option<&Property>, new: &Property);
}
