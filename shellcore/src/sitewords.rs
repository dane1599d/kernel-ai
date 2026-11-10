//! The words that name a site for the agent's network rows (P5 task 11, ADR
//! 0002 D3): the curated host table, and each installed app's word for the
//! hosts it declares. A fetch from a host one of the line's words names runs;
//! one from any other asks. An app's word joins when it is installed and
//! leaves when it is removed (`nethosts::Table`: no app shadows a curated
//! word or another app's).

use crate::Shell;

impl Shell {
    /// `app`'s word now names `hosts`, the hosts its signed manifest declares.
    /// All or nothing: a word the table holds already, or a bad host, refuses
    /// the lot.
    pub fn add_app_hosts(&mut self, app: &str, hosts: &[&str]) -> Result<(), nethosts::TableError> {
        self.hosts.add_app(app, hosts)
    }

    /// `app`'s word names nothing any more: it was removed. How many hosts it
    /// named.
    pub fn remove_app_hosts(&mut self, app: &str) -> usize {
        self.hosts.remove_app(app)
    }
}
