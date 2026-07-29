use sqlx::types::time::OffsetDateTime;

pub struct UserView {
    pub id: i32,
    pub provider: String,
    pub provider_subject: String,
    pub name: String,
    pub created_at: OffsetDateTime,
}

pub enum UserLoginForm {
    Google { provider: String, subject: String },
}

pub fn find_user(form: UserLoginForm) -> Option<UserView> {
    match form {
        UserLoginForm::Google { provider, subject } => None,
    }
}
