use std::{
    collections::HashMap,
    fs,
    io::Read,
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
    time::SystemTime
};
use fluent_bundle::{FluentArgs, FluentBundle, FluentResource};
use lazy_static::lazy_static;

use crate::{config, locale, log};

// Define mutable static values
lazy_static! {
    static ref TEMP_DIRECTORY: Mutex<Option<tempfile::TempDir>> = Mutex::new(None);
    static ref CACHE_DIRECTORY: Mutex<String> = Mutex::new(detect_directory());
    static ref STATUS: Mutex<String> = Mutex::new(locale::get_message(&locale::get_locale(None), "idling", None));
    static ref FILE_LIST: Mutex<Vec<AssetInfo>> = Mutex::new(Vec::new());
    static ref REQUEST_REPAINT: Mutex<bool> = Mutex::new(false);
    static ref PROGRESS: Mutex<f32> = Mutex::new(1.0);

    static ref LIST_TASK_RUNNING: Mutex<bool> = Mutex::new(false);
    static ref STOP_LIST_RUNNING: Mutex<bool> = Mutex::new(false);

    static ref FILTERED_FILE_LIST: Mutex<Vec<AssetInfo>> = Mutex::new(Vec::new());

    static ref TASK_RUNNING: Mutex<bool> = Mutex::new(false); // Delete/extract

    // File headers for each catagory
    static ref HEADERS: Mutex<HashMap<String,[String;2]>> = {
        let mut m = HashMap::new();
        m.insert("sounds".to_owned(),[
            "OggS".to_owned(),
            "ID3".to_owned()
            ]);
        m.insert("images".to_owned(), [
            "PNG".to_owned(),
            "WEBP".to_owned()
            ]);
        m.insert("ktx-files".to_owned(), [
            "KTX".to_owned(),
            "".to_owned()
            ]);
        m.insert("rbxm-files".to_owned(), [
            "<roblox!".to_owned(),
            "".to_owned()
            ]);
        Mutex::new(m)
    };

    // File extention for headers
    static ref EXTENTION: Mutex<HashMap<String, String>> = {
        let mut m = HashMap::new();
        m.insert("OggS".to_owned(), ".ogg".to_owned());
        m.insert("ID3".to_owned(), ".mp3".to_owned());
        m.insert("PNG".to_owned(), ".png".to_owned());
        m.insert("WEBP".to_owned(), ".webp".to_owned());
        m.insert("KTX".to_owned(), ".ktx".to_owned());
        m.insert("<roblox!".to_owned(), ".rbxm".to_owned());
        Mutex::new(m)
    };

    // Header offsets, headers that are not in this HashMap not be offset
    // Offset will subtract from the found header.
    static ref OFFSET: Mutex<HashMap<String, usize>> = {
        let mut m = HashMap::new();
        m.insert("PNG".to_owned(), 1);
        m.insert("KTX".to_owned(), 1);
        m.insert("WEBP".to_owned(), 8);
        Mutex::new(m)
    };
}


const DEFAULT_DIRECTORIES: [&str; 2] = ["%Temp%\\Roblox", "~/.var/app/org.vinegarhq.Sober/cache/sober"]; // For windows and linux (sober)

#[derive(Debug, Clone)]
pub struct AssetInfo {
    pub name: String,
    pub size: u64,
    pub last_modified: Option<SystemTime>
}

// Define local functions
fn update_status(value: String) {
    let mut status = STATUS.lock().unwrap();
    *status = value;
    let mut request = REQUEST_REPAINT.lock().unwrap();
    *request = true;
}

fn update_progress(value: f32) {
    let mut progress = PROGRESS.lock().unwrap();
    *progress = value;
    let mut request = REQUEST_REPAINT.lock().unwrap();
    *request = true;
}

fn update_file_list(value: AssetInfo, cli_list_mode: bool) {
    // cli_list_mode will print out to console
    // It is done this way so it can read files and print to console in the same stage
    if cli_list_mode {
        println!("{}", value.name);
    }
    let mut file_list = FILE_LIST.lock().unwrap();
    file_list.push(value)
}

fn clear_file_list() {
    let mut file_list = FILE_LIST.lock().unwrap();
    *file_list = Vec::new()
}

fn bytes_search(haystack: Vec<u8>, needle: &[u8]) -> Option<usize> {
    let len = needle.len();
    if len > 0 {
        haystack.windows(len).position(|window| window == needle)
    } else {
        None
    }
}

fn bytes_contains(haystack: &[u8], needle: &[u8]) -> bool {
    let len = needle.len();
    if len > 0 {
        if needle == b"ID3" {
            let bin_type = haystack.windows(7).any(|window| window == b"binary/");
            let mp3_header = haystack.windows(len).any(|window| window == needle);
            mp3_header && bin_type // Only allow mp3 if type is binary as that is what roblox uses
        } else {
            haystack.windows(len).any(|window| window == needle)
        }
    } else {
        false
    }

}

fn find_header(mode: String, bytes: Vec<u8>) -> String {
    // Get headers and offsets, they will be used later
    let all_headers = {
        HEADERS.lock().unwrap().clone()
    };

    // Get the header for the current mode
    let option_headers = all_headers.get(&mode);

    if let Some(headers) = option_headers {
        // Itearte through headers to find the correct one for this file.
        for header in headers {
            if bytes_contains(&bytes, header.as_bytes()) {
                return header.to_owned()
            }
        }
    }
    return "INVALID".to_owned()
}

fn extract_bytes(header: String, bytes: Vec<u8>) -> Vec<u8> {
    // Get offsets for headers
    let offsets = {
        OFFSET.lock().unwrap().clone()
    };

    // Find the header in the file
    if let Some(mut index) = bytes_search(bytes.clone(), header.as_bytes()) {
        // Found the header, extract from the bytes
        if let Some(offset) = offsets.get(&header) {
            // Apply offset to index if the offset exists
            index -= *offset;
        }
        // Return all the bytes after the found header index
        return bytes[index..].to_vec()
    }
    log::warn("Failed to extract a file!");
    // Return bytes instead if this fails
    return bytes
}

fn create_asset_info(path: &PathBuf, file: &str) -> AssetInfo {
    match fs::metadata(path) {
        Ok(metadata) => {
            let size = metadata.len();
            let last_modified = match metadata.modified() {
                Ok(system_time) => Some(system_time),
                Err(_) => None
            };

            return AssetInfo {
                name: file.to_string(),
                size: size,
                last_modified: last_modified
            }
        }
        Err(e) => {
            log::warn(&format!("Failed to get asset info: {}", e));
            return AssetInfo {
                name: file.to_string(),
                size: 0,
                last_modified: None
            }
        }
    }
}

fn create_no_files(locale: &FluentBundle<Arc<FluentResource>>) -> AssetInfo {
    AssetInfo {
        name: locale::get_message(&locale, "no-files", None),
        size: 0,
        last_modified: None
    }
}

// Define public functions
pub fn validate_directory(directory: &str) -> Result<String, String> {
    let resolved_directory = resolve_path(directory);
    // There's probably a better way of doing this... It works though :D

    match fs::metadata(&resolved_directory) { // Directory detection
        Ok(metadata) => {
            if metadata.is_dir() {
                // Successfully detected a directory, we can return it
                return Ok(resolved_directory);
            } else {
                return Err(format!("{}: Not a directory", resolved_directory));
            }
        }
        Err(e) => {
            return Err(e.to_string()); // Convert to correct data type
        }
    }
}

pub fn resolve_path(directory: &str) -> String {
    let resolved_path = directory
    .replace("%Temp%", &format!("C:\\Users\\{}\\AppData\\Local\\Temp", whoami::username()))
    .replace("%localappdata%", &format!("C:\\Users\\{}\\AppData\\Local", whoami::username()))
    .replace("~", &format!("/home/{}", whoami::username()));
    // There's probably a better way of doing this... It works though :D
    return resolved_path
}

pub fn detect_directory() -> String {
    let mut errors = "".to_owned();
    if let Some(directory) = config::get_config().get("cache_directory") {
        // User-specified directory from config
        match validate_directory(&directory.to_string().replace('"',"")) { // It kept returning "value" instead of value
            Ok(resolved_directory) => return resolved_directory,
            Err(e) => {
                errors.push_str(&e.to_string());
            },
        }
    }
    // Directory detection
    for directory in DEFAULT_DIRECTORIES {
        match validate_directory(directory) {
            Ok(resolved_directory) => return resolved_directory,
            Err(e) => errors.push_str(&e.to_string()),
        }  

    }

    // If it was unable to detect any directory, tell the user
    let _ = native_dialog::MessageDialog::new()
    .set_type(native_dialog::MessageType::Error)
    .set_title(&locale::get_message(&locale::get_locale(None), "error-directory-detection-title", None))
    .set_text(&locale::get_message(&locale::get_locale(None), "error-directory-detection-description", None))
    .show_alert();

    let yes = native_dialog::MessageDialog::new()
    .set_type(native_dialog::MessageType::Error)
    .set_title(&locale::get_message(&locale::get_locale(None), "confirmation-custom-directory-title", None))
    .set_text(&locale::get_message(&locale::get_locale(None), "confirmation-custom-directory-description", None))
    .show_confirm()
    .unwrap();

    if yes {
        let option_path = native_dialog::FileDialog::new()
        .show_open_single_dir()
        .unwrap();
        if let Some(path) = option_path {
            config::set_config_value("cache_directory", validate_directory(&path.to_string_lossy().to_string()).unwrap().into());
            return detect_directory();
        } else {
            panic!("Directory detection failed!{}", errors);
        }
    } else {
        panic!("Directory detection failed!{}", errors);
    }
}

// Function to get temp directory, create it if it doesn't exist
pub fn get_temp_dir(create_directory: bool) -> String {
    let mut option_temp_dir = TEMP_DIRECTORY.lock().unwrap();
    if let Some(temp_dir) = option_temp_dir.as_ref() {
        return temp_dir.path().to_string_lossy().to_string();
    } else if create_directory  {
        match tempfile::tempdir() {
            Ok(temp_dir) => {
                let path = temp_dir.path().to_string_lossy().to_string();
                *option_temp_dir = Some(temp_dir);
                return path;
            }
            Err(e) => {
                // Have a visual dialog to show the user what actually went wrong
                let _ = native_dialog::MessageDialog::new()
                .set_type(native_dialog::MessageType::Error)
                .set_title(&locale::get_message(&locale::get_locale(None), "error-temporary-directory-title", None))
                .set_text(&locale::get_message(&locale::get_locale(None), "error-temporary-directory-description", None))
                .show_alert();
                panic!("Failed to create a temporary directory! {}", e)
            }
        }
    } else {
        return "".to_string();
    }
}


pub fn delete_all_directory_contents(dir: String) {
    if dir == "" {
        panic!("Panic!ed due to safety. cache_directory was blank! Can possibly DELETE EVERYTHING!")
    }
    // Bunch of error checking to check if it's a valid directory
    match fs::metadata(dir.clone()) {
        Ok(metadata) => {
            if metadata.is_dir() {
                let running = {
                    let task = TASK_RUNNING.lock().unwrap();
                    task.clone()
                };
                // Stop multiple threads from running
                if running == false {
                    thread::spawn(|| {
                        { 
                            let mut task = TASK_RUNNING.lock().unwrap();
                            *task = true; // Stop other threads from running
                        }
                        // Get locale for localised status messages
                        let locale = locale::get_locale(None);
                        
                        // Read directory
                        let entries: Vec<_> = fs::read_dir(dir).unwrap().collect();

                        // Get amount and initlilize counter for progress
                        let total = entries.len();
                        let mut count = 0;

                        for entry in entries {
                            count += 1; // Increase counter for progress
                            update_progress(count as f32/total as f32); // Convert to f32 to allow floating point output
                            let path = entry.unwrap().path();

                            // Args for formatting
                            let mut args = FluentArgs::new();
                            args.set("item", count);
                            args.set("total", total);
                            if path.is_dir() {
                                match fs::remove_dir_all(path) {
                                    // Error handling and update status
                                    Ok(_) => update_status(locale::get_message(&locale, "deleting-files", Some(&args))),

                                    // If it's an error, log it and show on GUI
                                    Err(e) => {
                                        log::error(&format!("Failed to delete file: {}: {}", count, e));
                                        update_status(locale::get_message(&locale, "failed-deleting-file", Some(&args)));
                                    }
                                }
                            } else {
                                match fs::remove_file(path) {
                                    // Error handling and update status
                                    Ok(_) => update_status(locale::get_message(&locale, "deleting-files", Some(&args))),
    
                                    // If it's an error, log it and show on GUI
                                    Err(e) => {
                                        log::error(&format!("Failed to delete file: {}: {}", count, e));
                                        update_status(locale::get_message(&locale, "failed-deleting-file", Some(&args)));
                                    }
                                }    
                            }
                        
                            
                        }
                        // Clear the file list for visual feedback to the user that the files are actually deleted
                        clear_file_list();
                        
                        update_file_list(create_no_files(&locale), false);
                        { 
                            let mut task = TASK_RUNNING.lock().unwrap();
                            *task = false; // Allow other threads to run again
                        }
                        update_status(locale::get_message(&locale, "idling", None)); // Set the status back
                    });
                }
            // Error handling just so the program doesn't crash for seemingly no reason
            } else {
                update_status(locale::get_message(&locale::get_locale(None), "error-check-logs", None)); 
                log::error("ERROR: Directory detection failed.")
            }
        }
        Err(e) => {
            log::warn(&format!("WARN: '{}' {}", dir, e));
            update_status(locale::get_message(&locale::get_locale(None), "idling", None)); 
        }
    }
}

pub fn refresh(dir: String, mode: String, cli_list_mode: bool, yield_for_thread: bool) {
    // Bunch of error checking to check if it's a valid directory
    match fs::metadata(dir.clone()) {
        Ok(metadata) => {
            if metadata.is_dir() {
                
                let handle = thread::spawn(move || {
                    // Get locale for localised status messages
                    let locale = locale::get_locale(None);
                    // This loop here is to make it wait until it is not running, and to set the STOP_LIST_RUNNING to true if it is running to make the other thread
                    loop {
                        let running = {
                            let task = LIST_TASK_RUNNING.lock().unwrap();
                            task.clone()
                        };
                        if !running {
                            break // Break if not running
                        } else {
                            let mut stop = STOP_LIST_RUNNING.lock().unwrap(); // Tell the other thread to stop
                            *stop = true;
                        }
                        thread::sleep(std::time::Duration::from_millis(10)); // Sleep for a bit to not be CPU intensive
                    }
                    { 
                        let mut task = LIST_TASK_RUNNING.lock().unwrap();
                        *task = true; // Tell other threads that a task is running
                        let mut stop = STOP_LIST_RUNNING.lock().unwrap();
                        *stop = false; // Disable the stop, otherwise this thread will stop!
                    }

                    clear_file_list(); // Only list the files on the current tab

                    // Read directory
                    let entries: Vec<_> = fs::read_dir(dir).unwrap().collect();

                    // Get amount and initlilize counter for progress
                    let total = entries.len();
                    let mut count = 0;

                    // Tell the user that there is no files to list to make it easy to tell that the program is working and it isn't broken
                    if total == 0 {
                        update_file_list(AssetInfo {
                            name: locale::get_message(&locale, "no-files", None),
                            size: 0,
                            last_modified: None
                        }, cli_list_mode);
                    }

                    if mode != "music" { // Music lists files directly and others filter.
                        // Filter the files out
                        let all_headers = {
                            HEADERS.lock().unwrap().clone()
                        };
                        
                        let headers = if let Some(value) = all_headers.get(&mode) {
                            value
                        } else {
                            return
                        };

                        for entry in entries {
                            let stop = {
                                let stop_task = STOP_LIST_RUNNING.lock().unwrap();
                                stop_task.clone()
                            };
                            if stop {
                                break // Stop if another thread requests to stop this task.
                            }
                            
                            count += 1; // Increase counter for progress
                            update_progress(count as f32/total as f32); // Convert to f32 to allow floating point output
                            let path = entry.unwrap().path();
                            let display = path.display();

                            // Args for formatting
                            let mut args = FluentArgs::new();
                            args.set("item", count);
                            args.set("total", total);
    
                            if let Some(filename) = path.file_name() {
                                match &mut fs::File::open(&path) {
                                    Err(why) => {
                                        log::error(&format!("Couldn't open {}: {}", display, why));
                                        args.set("error", why.to_string());
                                        update_status(locale::get_message(&locale, "failed-opening-file", Some(&args)));
                                    },
                                    Ok(file) => {
                                        // Reading the first 2048 bytes
                                        let mut buffer = vec![0; 2048];
                                        match file.read(&mut buffer) {
                                            Err(why) => {
                                                log::error(&format!("Couldn't open {}: {}", display, why));
                                                update_status(locale::get_message(&locale, "failed-opening-file", Some(&args)));
                                            },
                                            Ok(bytes_read) => {
                                                buffer.truncate(bytes_read);
                                                for header in headers {
                                                    // Check if header is empty before actually checking file
                                                    if header != "" {
                                                        // Add the file if the file contains the header
                                                        if bytes_contains(&buffer, header.as_bytes()) {
                                                            update_file_list(create_asset_info(&path, &filename.to_string_lossy()), false);
                                                        }
                                                    }
      
                                                }

                                                update_status(locale::get_message(&locale, "reading-files", Some(&args)));
                                            }
                                        }
                                        
                                    },
                                };
                            }
                        }
                    } else {
                        // List the files from the directory instead of filtering
                        for entry in entries {
                            let stop = {
                                let stop_task = STOP_LIST_RUNNING.lock().unwrap();
                                stop_task.clone()
                            };
                            if stop {
                                break // Stop if another thread requests to stop this task.
                            }
                            
                            count += 1; // Increase counter for progress
                            update_progress(count as f32/total as f32);
                            let path = entry.unwrap().path();
                            if let Some(filename) = path.file_name() {
                                update_file_list(create_asset_info(&path, &filename.to_string_lossy()), cli_list_mode);
                                
                                let mut args = FluentArgs::new();
                                args.set("item", count);
                                args.set("total", total);
                                update_status(locale::get_message(&locale, "reading-files", Some(&args)));
                            }
                            
                        }
                    }


                    { 
                        let mut task = LIST_TASK_RUNNING.lock().unwrap();
                        *task = false; // Allow other threads to run again
                    }
                    update_status(locale::get_message(&locale, "idling", None)); // Set the status back
                });

                if yield_for_thread {
                    // Will wait for the thread instead of quitting immediately
                    let _ = handle.join();
                }
            // Error handling just so the program doesn't crash for seemingly no reason
            } else {
                let mut status = STATUS.lock().unwrap();
                *status = format!("Error: check logs for more details.");
                log::error(&format!("ERROR: Directory detection failed."))
            }
        }
        Err(e) => {
            log::warn(&format!("'{}' {}", dir, e));
            clear_file_list();
            update_file_list(create_no_files(&locale::get_locale(None)), cli_list_mode);
            update_status(locale::get_message(&locale::get_locale(None), "idling", None));
        }
    }
}

pub fn extract_file(file: String, mode: String, destination: String, add_extention: bool) -> String {
    match fs::metadata(file.clone()) {
        Ok(metadata) => {
            if metadata.is_file() {
                // This can return an error result
                let bytes_error = fs::read(file);
                match bytes_error {
                    // Remove the error result so the extract_bytes function can read it
                    Ok(bytes) => {
                        let header = find_header(mode, bytes.clone());
                        let extracted_bytes = if header != "INVALID" {
                            extract_bytes(header.clone(), bytes.clone())
                        } else {
                            bytes.clone()
                        };

                        let mut new_destination = destination.clone();

                        // Add the extention if needed
                        if add_extention {
                            let extentions = {EXTENTION.lock().unwrap().clone()};
                            if let Some(extention) = extentions.get(&header.clone()) {
                                new_destination = destination.clone() + &extention.clone()
                            } else {
                                new_destination = destination.clone() + ".ogg" // Music tab
                            }
                        }

                        match fs::write(new_destination.clone(), extracted_bytes) {
                            Ok(_) => (),
                            Err(e) => log::error(&format!("Error writing file: {}", e)),
                        }

                        if let Ok(sys_modified_time) = metadata.modified() {
                            let modified_time = filetime::FileTime::from_system_time(sys_modified_time);
                            match filetime::set_file_times(&new_destination, modified_time, modified_time) {
                                Ok(_) => (),
                                Err(e) => log::error(&format!("Failed to write file modification time {}", e))
                            }
                        }                        

                        return new_destination;


                    }
                    Err(e) => {
                        update_status(locale::get_message(&locale::get_locale(None), "failed-opening-file", None));
                        log::error(&format!("Failed to open file: {}", e));
                        return "None".to_string();
                    }
                }
            // Error handling just so the program doesn't crash for seemingly no reason
            } else {
                // Args for formatting
                let mut args = FluentArgs::new();
                args.set("file", &file);

                update_status(locale::get_message(&locale::get_locale(None), "failed-not-file", Some(&args)));
                log::error(&format!(" '{}' Not a file.", file));
                return "None".to_string();
            }
        }
        Err(e) => {
            // Args for formatting
            let mut args = FluentArgs::new();
            args.set("error", e.to_string());

            log::error(&format!("Error extracting file: '{}' {}", file, e));
            update_status(locale::get_message(&locale::get_locale(None), "idling", Some(&args)));
            return "None".to_string();
        }
    }
}

pub fn extract_file_to_bytes(file: &str, mode: &str) -> Vec<u8> {
    match fs::metadata(file) {
        Ok(metadata) => {
            if metadata.is_file() {
                // This can return an error result
                let bytes_error = fs::read(file);
                match bytes_error {
                    // Remove the error result so the extract_bytes function can read it
                    Ok(bytes) => {
                        let header = find_header(mode.to_string(), bytes.clone());
                        let extracted_bytes = if header != "INVALID" {
                            extract_bytes(header.clone(), bytes.clone())
                        } else {
                            bytes.clone()
                        };

                        return extracted_bytes;

                    }
                    Err(e) => {
                        update_status(locale::get_message(&locale::get_locale(None), "failed-opening-file", None));
                        log::error(&format!("Failed to open file: {}", e));
                        return "None".as_bytes().to_vec();
                    }
                }
            // Error handling just so the program doesn't crash for seemingly no reason
            } else {
                // Args for formatting
                let mut args = FluentArgs::new();
                args.set("file", file);

                update_status(locale::get_message(&locale::get_locale(None), "failed-not-file", Some(&args)));
                log::error(&format!(" '{}' Not a file.", file));
                return "None".as_bytes().to_vec();
            }
        }
        Err(e) => {
            // Args for formatting
            let mut args = FluentArgs::new();
            args.set("error", e.to_string());

            log::error(&format!("Error extracting file: '{}' {}", file, e));
            update_status(locale::get_message(&locale::get_locale(None), "idling", Some(&args)));
            return "None".as_bytes().to_vec();
        }
    }
}


pub fn extract_dir(dir: String, destination: String, mode: String, yield_for_thread: bool, use_alias: bool) {
    // Create directory if it doesn't exist
    match fs::create_dir(destination.clone()) {
        Ok(_) => (),
        Err(e) => log::error(&format!("Error creating directory: {}", e))
    };
    // Bunch of error checking to check if it's a valid directory
    match fs::metadata(dir.clone()) {
        Ok(metadata) => {
            if metadata.is_dir() {
                let running = {
                    let task = TASK_RUNNING.lock().unwrap();
                    task.clone()
                };
                // Stop multiple threads from running
                if running == false {
                    let handle = thread::spawn(move || {
                        { 
                            let mut task = TASK_RUNNING.lock().unwrap();
                            *task = true; // Stop other threads from running
                        }

                        // User has configured it to refresh before extracting
                        if config::get_config_bool("refresh_before_extract").unwrap_or(false) {
                            refresh(dir.clone(), mode.clone(), false, true); // true because it'll run both and have unfinished file list
                        }

                        let file_list = get_file_list();

                        // Get locale for localised status messages
                        let locale = locale::get_locale(None);

                        // Get amount and initlilize counter for progress
                        let total = file_list.len();
                        let mut count = 0;

                        for entry in file_list {
                            count += 1; // Increase counter for progress
                            update_progress(count as f32/total as f32); // Convert to f32 to allow floating point output
                            let origin = format!("{}/{}", dir, entry.name);

                            let alias = if use_alias {
                                config::get_asset_alias(&entry.name)
                            } else {
                                entry.name
                            };

                            let dest = format!("{}/{}", destination, alias); // Local variable destination

                            // Args for formatting
                            let mut args = FluentArgs::new();
                            args.set("item", count);
                            args.set("total", total);

                            let result = extract_file(origin, mode.clone(), dest, true);
                            if result == "None" {
                                update_status(locale::get_message(&locale, "failed-extracting-file", Some(&args)));
                            } else {
                                update_status(locale::get_message(&locale, "extracting-files", Some(&args)));
                            }
                        
                            
                        }
                        { 
                            let mut task = TASK_RUNNING.lock().unwrap();
                            *task = false; // Allow other threads to run again
                        }
                        update_status(locale::get_message(&locale, "all-extracted", None)); // Set the status to confirm to the user that all has finished
                    });
                    
                    if yield_for_thread {
                        // Will wait for the thread instead of quitting immediately
                        let _ = handle.join();
                    }
                }
            // Error handling just so the program doesn't crash for seemingly no reason
            } else {
                update_status(locale::get_message(&locale::get_locale(None), "error-check-logs", None)); 
                log::error(&format!(" Directory detection failed."))
            }
        }
        Err(e) => {
            log::warn(&format!("'{}' {}", dir, e));
            update_status(locale::get_message(&locale::get_locale(None), "idling", None));
        }
    }
}

pub fn extract_all(destination: String, yield_for_thread: bool, use_alias: bool) {
    let running = {
        let task = TASK_RUNNING.lock().unwrap();
        task.clone()
    };
    // Stop multiple threads from running
    if running == false {
        let handle = thread::spawn(move || {
            { 
                let mut task = TASK_RUNNING.lock().unwrap();
                *task = true; // Stop other threads from running
            }

            // Get locale for localised status messages
            let locale = locale::get_locale(None);

            let headers = {HEADERS.lock().unwrap().clone()};
            let mut all_headers: Vec<(String, String)> = Vec::new();

            for key in headers.keys() {
                if let Some(mode_headers) = headers.get(key) {
                    for single_header in mode_headers {
                        all_headers.push((single_header.to_string(), key.to_string()));
                    }
                }
            }

            let cache_directory = get_cache_directory();
            let music_directory = format!("{}/sounds", cache_directory);
            let http_directory = format!("{}/http", cache_directory);

            // Attempt to create directories
            let _ = fs::create_dir(destination.clone());
            let _ = fs::create_dir(format!("{}/Music", destination.clone()));
            
            // Loop through all types and create directories for them
            for key in headers.keys() {
                let _ = fs::create_dir(format!("{}/{}", destination.clone(), key));
            }

            // Stage 1: Read and extract music directory
            let entries: Vec<_> = fs::read_dir(music_directory.clone()).unwrap().collect();

            // Get amount and initlilize counter for progress
            let total = entries.len();
            let mut count = 0;
            for entry in entries {                            
                count += 1; // Increase counter for progress
                update_progress((count as f32/total as f32)/ 3.0);

                // Args for formatting
                let mut args = FluentArgs::new();
                args.set("item", count);
                args.set("total", total);

                let path = entry.unwrap().path();
                if let Some(filename) = path.file_name() {
                    let name = filename.to_string_lossy().to_string();
                    let origin = format!("{}/{}", music_directory.clone(), name);

                    let alias = if use_alias {
                        config::get_asset_alias(&name)
                    } else {
                        name
                    };


                    let dest = format!("{}/Music/{}", destination, alias); // Local destination
                    extract_file(origin, "Music".to_string(), dest, true);

                    // More formatting to show "Stage 1/3: Extracting files"
                    args.set("status", locale::get_message(&locale, "extracting-files", Some(&args)));
                    args.set("stage", "1");
                    args.set("max", "3");

                    update_status(locale::get_message(&locale, "stage", Some(&args)));
                }
            }

            // Stage 2: Filter the files
            let entries: Vec<_> = fs::read_dir(http_directory.clone()).unwrap().collect();

            // Initilize the Vec for the filtered files to go in
            let mut filtered_files: Vec<(String, String)> = Vec::new();

            // Get amount and initlilize counter for progress
            let total = entries.len();
            let mut count = 0;
            for entry in entries {                            
                count += 1; // Increase counter for progress
                update_progress(((count as f32/total as f32) +1.0) /3.0); // 2nd stage, will fill up the bar from 1/3 to 2/3

                // Args for formatting
                let mut args = FluentArgs::new();
                args.set("item", count);
                args.set("total", total);

                let path = entry.unwrap().path();
                if let Some(filename) = path.file_name() {
                    match &mut fs::File::open(&path) {
                        Err(why) => {
                            log::error(&format!("Couldn't open file: {}", why));
                            update_status(locale::get_message(&locale, "failed-opening-file", Some(&args)));
                        },
                        Ok(file) => {
                            // Reading the first 2048 bytes
                            let mut buffer = vec![0; 2048];
                            match file.read(&mut buffer) {
                                Err(why) => {
                                    log::error(&format!("Couldn't open file: {}", why));
                                    update_status(locale::get_message(&locale, "failed-opening-file", Some(&args)));
                                },
                                Ok(bytes_read) => {
                                    buffer.truncate(bytes_read);
                                    // header.0 = header, header.1 = mode
                                    for header in all_headers.clone() {
                                        // Check if header is not empty before actually checking file
                                        if header.0 != "" {
                                            // Add it to the list if the header is inside of the file.
                                            if bytes_contains(&buffer, header.0.as_bytes()) {                                        
                                                filtered_files.push((filename.to_string_lossy().to_string(), header.1))
                                            }
                                        }

                                    }

                                    // More formatting to show "Stage 2/3: Filtering files"
                                    args.set("status", locale::get_message(&locale, "filtering-files", Some(&args)));
                                    args.set("stage", "2");
                                    args.set("max", "3");

                                    update_status(locale::get_message(&locale, "stage", Some(&args)));
                                }
                            }
                            
                        },
                    };
                }
            }

            // Stage 3: Extract the files

            // Get amount and initlilize counter for progress
            let total = filtered_files.len();
            let mut count = 0;
            for file in filtered_files {
                count += 1; // Increase counter for progress
                update_progress(((count as f32/total as f32) +2.0) /3.0); // 3rd stage, will fill up the bar from 2/3 to 3/3

                // Args for formatting
                let mut args = FluentArgs::new();
                args.set("item", count);
                args.set("total", total);

                let origin = format!("{}/{}", http_directory, file.0);
                
                let alias = if use_alias {
                    config::get_asset_alias(&file.0)
                } else {
                    file.0
                };

                let dest = format!("{}/{}/{}", destination, file.1, alias); // Local destination, stores in (destination/type/name)
                extract_file(origin, file.1, dest, true);

                // More formatting to show "Stage 3/3: Extracting files"
                args.set("status", locale::get_message(&locale, "extracting-files", Some(&args)));
                args.set("stage", "3");
                args.set("max", "3");

                update_status(locale::get_message(&locale, "stage", Some(&args)));
            }

            { 
                let mut task = TASK_RUNNING.lock().unwrap();
                *task = false; // Allow other threads to run again
            }
            update_status(locale::get_message(&locale, "all-extracted", None)); // Set the status to confirm to the user that all has finished
        });
        
        if yield_for_thread {
            // Will wait for the thread instead of quitting immediately
            let _ = handle.join();
        }
    }
}

pub fn swap_assets(dir: &str, asset_a: &str, asset_b: &str) {
    let asset_a_path = format!("{}/{}", dir, asset_a);
    let asset_b_path = format!("{}/{}", dir, asset_b);
    let locale = locale::get_locale(None);

    let asset_a_bytes = match fs::read(&asset_a_path) {
        Ok(bytes) => {
            bytes
        },
        Err(e) => {
            let mut args= FluentArgs::new();
            args.set("error", e.to_string());

            update_status(locale::get_message(&locale, "failed-opening-file", Some(&args)));
            log::error(&format!("Error opening file '{}': {}", asset_a_path, e));
            return
        }
    };

    let asset_b_bytes = match fs::read(&asset_b_path) {
        Ok(bytes) => {
            bytes
        },
        Err(e) => {
            let mut args= FluentArgs::new();
            args.set("error", e.to_string());

            update_status(locale::get_message(&locale, "failed-opening-file", Some(&args)));
            log::error(&format!("Error opening file '{}': {}", asset_b_path, e));
            return
        }
    };

    match fs::write(&asset_a_path, asset_b_bytes) {
        Ok(_) => (),
        Err(e) => {
            let mut args= FluentArgs::new();
            args.set("error", e.to_string());

            update_status(locale::get_message(&locale, "failed-opening-file", Some(&args)));
            log::error(&format!("Error opening file '{}': {}", asset_a_path, e));
        }
    };

    match fs::write(&asset_b_path, asset_a_bytes) {
        Ok(_) => (),
        Err(e) => {
            let mut args= FluentArgs::new();
            args.set("error", e.to_string());

            update_status(locale::get_message(&locale, "failed-opening-file", Some(&args)));
            log::error(&format!("Error opening file '{}': {}", asset_b_path, e));
        }
    };
    let mut args= FluentArgs::new();
    args.set("item_a", asset_a);
    args.set("item_b", asset_b);
    update_status(locale::get_message(&locale, "swapped", Some(&args)));
}

pub fn copy_assets(dir: &str, asset_a: &str, asset_b: &str) {
    let asset_a_path = format!("{}/{}", dir, asset_a);
    let asset_b_path = format!("{}/{}", dir, asset_b);
    let locale = locale::get_locale(None);

    let asset_a_bytes = match fs::read(&asset_a_path) {
        Ok(bytes) => {
            bytes
        },
        Err(e) => {
            let mut args= FluentArgs::new();
            args.set("error", e.to_string());

            update_status(locale::get_message(&locale, "failed-opening-file", Some(&args)));
            log::error(&format!("Error opening file '{}': {}", asset_a_path, e));
            return
        }
    };

    match fs::write(&asset_b_path, asset_a_bytes) {
        Ok(_) => (),
        Err(e) => {
            let mut args= FluentArgs::new();
            args.set("error", e.to_string());

            update_status(locale::get_message(&locale, "failed-opening-file", Some(&args)));
            log::error(&format!("Error opening file '{}': {}", asset_b_path, e));
        }
    };

    let mut args= FluentArgs::new();
    args.set("item_a", asset_a);
    args.set("item_b", asset_b);
    update_status(locale::get_message(&locale, "copied", Some(&args)));
}

pub fn filter_file_list(query: String) {
    let query_lower = query.to_lowercase();
    // Clear file list before
    {
        let mut filtered_file_list = FILTERED_FILE_LIST.lock().unwrap();
        *filtered_file_list = Vec::new();
    }
    let file_list = get_file_list(); // Clone file list
    for file in file_list {
        if file.name.contains(&query_lower) || config::get_asset_alias(&file.name).to_lowercase().contains(&query_lower) {
            {
                let mut filtered_file_list = FILTERED_FILE_LIST.lock().unwrap();
                filtered_file_list.push(file);
            }
        }
    }
}

pub fn get_file_list() -> Vec<AssetInfo> {
    FILE_LIST.lock().unwrap().clone()
}

pub fn get_filtered_file_list() -> Vec<AssetInfo> {
    FILTERED_FILE_LIST.lock().unwrap().clone()
}

pub fn get_cache_directory() -> String {
    CACHE_DIRECTORY.lock().unwrap().clone()
}

pub fn set_cache_directory(value: String) {
    let mut cache_directory = CACHE_DIRECTORY.lock().unwrap();
    *cache_directory = value;
}

pub fn get_status() -> String {
    STATUS.lock().unwrap().clone()
}

pub fn get_progress() -> f32 {
    PROGRESS.lock().unwrap().clone()
}

pub fn get_list_task_running() -> bool {
    LIST_TASK_RUNNING.lock().unwrap().clone()
}

pub fn get_request_repaint() -> bool {
    let mut request_repaint = REQUEST_REPAINT.lock().unwrap();
    let old_request_repaint = *request_repaint;
    *request_repaint = false; // Set to false when this function is called to acknoledge
    return old_request_repaint
}

pub fn get_categories() -> Vec<String> {
    let mut catagories = Vec::new();
    for key in HEADERS.lock().unwrap().keys() {
        catagories.push(key.to_owned());
    }
    return catagories;
}

// Delete the temp directory
pub fn clean_up() {
    let temp_dir = get_temp_dir(false);
    // Just in case if it somehow resolves to "/"
    if temp_dir != "" && temp_dir != "/" {
        log::info(&format!("Cleaning up {}", temp_dir));
        let _ = fs::remove_dir_all(temp_dir); // Not too important, ignore value, and the last thing the program will run
    }
}