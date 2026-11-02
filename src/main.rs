#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

#[cfg(target_os = "windows")]
mod app;
#[cfg(target_os = "windows")]
mod diagnostics;
#[cfg(target_os = "windows")]
mod overlay;
#[cfg(target_os = "windows")]
mod settings;
#[cfg(target_os = "windows")]
mod single_instance;

#[cfg(target_os = "windows")]
fn main() {
    let restricted_dll_search = rustshot::platform::windows::loader::restrict_dll_search();
    diagnostics::initialize();
    if let Err(error) = restricted_dll_search {
        diagnostics::record_error("Не удалось ограничить поиск DLL", &error.to_string());
    }
    install_panic_dialog();
    let _single_instance = match single_instance::SingleInstance::acquire() {
        Ok(Some(instance)) => instance,
        Ok(None) => {
            rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Info)
                .set_title("Rustshot уже запущен")
                .set_description(
                    "Rustshot уже запущен в этом сеансе Windows. Используйте его значок в трее или горячие клавиши.",
                )
                .show();
            return;
        }
        Err(error) => {
            diagnostics::record_error(
                "Не удалось проверить, запущен ли Rustshot",
                &error.to_string(),
            );
            rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Error)
                .set_title("Не удалось запустить Rustshot")
                .set_description(error.to_string())
                .show();
            std::process::exit(1);
        }
    };
    if let Err(error) = app::run() {
        diagnostics::record_error("Не удалось запустить Rustshot", &error.to_string());
        rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Error)
            .set_title("Не удалось запустить Rustshot")
            .set_description(error.to_string())
            .show();
        std::process::exit(1);
    }
}

#[cfg(target_os = "windows")]
fn install_panic_dialog() {
    std::panic::set_hook(Box::new(|panic| {
        diagnostics::record_error("Непредвиденный сбой", &panic.to_string());
        rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Error)
            .set_title("В Rustshot произошла непредвиденная ошибка")
            .set_description(panic.to_string())
            .show();
    }));
}

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("Rustshot's first release currently supports Windows only.");
}
