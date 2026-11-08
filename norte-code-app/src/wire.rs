use serde::Serialize;

#[derive(Serialize)]
pub struct UserInputRequest<'a> {
    pub prompt: &'a str,
}
