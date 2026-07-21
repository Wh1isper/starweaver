use tauri::{
    AppHandle, Manager, RunEvent, Runtime,
    plugin::{self, TauriPlugin},
};
use zbus::{blocking::Connection, interface, names::WellKnownName};

use super::ActivationCallback;

struct ConnectionHandle(Connection);
struct DbusName(String);

struct ActivationService<R: Runtime> {
    app: AppHandle<R>,
    callback: Box<ActivationCallback<R>>,
}

#[interface(name = "io.github.wh1isper.Starweaver.SingleInstance")]
impl<R: Runtime> ActivationService<R> {
    fn activate(&self) {
        (self.callback)(&self.app);
    }
}

pub fn init<R: Runtime>(callback: Box<ActivationCallback<R>>) -> TauriPlugin<R> {
    plugin::Builder::new("starweaver-single-instance")
        .setup(move |app, _api| {
            let dbus_name = format!("{}.SingleInstance", app.config().identifier);
            let dbus_path = format!("/{}", dbus_name.replace('.', "/").replace('-', "_"));
            let service = ActivationService {
                app: app.clone(),
                callback,
            };
            let builder = zbus::blocking::connection::Builder::session()?
                .name(dbus_name.as_str())?
                .replace_existing_names(false)
                .allow_name_replacements(false)
                .serve_at(dbus_path.as_str(), service)?;

            match builder.build() {
                Ok(connection) => {
                    app.manage(ConnectionHandle(connection));
                }
                Err(zbus::Error::NameTaken) => {
                    let connection = Connection::session()?;
                    connection.call_method(
                        Some(dbus_name.as_str()),
                        dbus_path.as_str(),
                        Some("io.github.wh1isper.Starweaver.SingleInstance"),
                        "Activate",
                        &(),
                    )?;
                    app.cleanup_before_exit();
                    std::process::exit(0);
                }
                Err(error) => return Err(error.into()),
            }

            app.manage(DbusName(dbus_name));
            Ok(())
        })
        .on_event(|app, event| {
            if matches!(event, RunEvent::Exit) {
                destroy(app);
            }
        })
        .build()
}

fn destroy<R: Runtime, M: Manager<R>>(manager: &M) {
    let Some(connection) = manager.try_state::<ConnectionHandle>() else {
        return;
    };
    let Some(name) = manager
        .try_state::<DbusName>()
        .and_then(|name| WellKnownName::try_from(name.0.clone()).ok())
    else {
        return;
    };
    let _ = connection.0.release_name(name);
}
