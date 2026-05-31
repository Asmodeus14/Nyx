import socket
import struct
import ctypes
import pygame
import threading
import os

# --- THE WINDOWS BLUR FIX ---
try:
    ctypes.windll.shcore.SetProcessDpiAwareness(2)
except Exception:
    pass

UDP_IP = "192.168.137.1"
UDP_PORT = 8080
CHUNK_SIZE = 1400
HEADER_SIZE = 24

# --- MULTI-THREADING SHARED MEMORY ---
frame_lock = threading.Lock()
render_buffer = bytearray()
frame_ready = False
frame_w, frame_h, frame_stride = 0, 0, 0

def is_hex_string(s):
    """Checks if the string contains only valid hex characters."""
    try:
        int(s, 16)
        return True
    except ValueError:
        return False

def network_daemon():
    """Runs on a background thread. Drains the network card at maximum speed."""
    global render_buffer, frame_ready, frame_w, frame_h, frame_stride
    
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 1024 * 1024 * 20)
    sock.bind((UDP_IP, UDP_PORT))
    
    back_buffer = bytearray()
    highest_frame_id = -1 
    w, h, stride = 0, 0, 0
    expected_total_chunks = 0
    packets_received = 0

    print(f"[*] Network Daemon Listening on {UDP_IP}:{UDP_PORT}...")

    while True:
        try:
            data, addr = sock.recvfrom(2048)
            
            # --- NEW: DSDT EXFILTRATOR DETECTION ---
            # Attempt to decode as ASCII. Video stream packets will fail this or the hex check.
            try:
                text_data = data.decode('ascii')
                if is_hex_string(text_data):
                    # It's a DSDT Payload!
                    
                    # The ACPI DSDT header starts with "DSDT" which is "44534454" in hex.
                    # If we see the signature, we start a fresh file.
                    mode = "w" if text_data.startswith("44534454") else "a"
                    
                    with open("dsdt.hex", mode) as f:
                        f.write(text_data)
                    
                    if mode == "w":
                        print("\n[+] INCOMING DSDT PAYLOAD DETECTED! Resetting dsdt.hex...")
                    print(f"    -> Appended {len(text_data)} hex bytes.")
                    
                    # Skip the video processing for this text packet!
                    continue 
            except UnicodeDecodeError:
                pass # Not ASCII text, proceed to video stream logic
            # ---------------------------------------

            if len(data) > 500:
                c_idx, t_chunks, f_id, f_w, f_h, f_stride = struct.unpack('<IIIIII', data[:HEADER_SIZE])
                
                # --- THE TIME WARP FIX ---
                # If this packet is from the past, drop it!
                if highest_frame_id != -1 and f_id < highest_frame_id and (highest_frame_id - f_id) < 100000:
                    continue 
                
                if highest_frame_id == -1 or f_id > highest_frame_id:
                    highest_frame_id = f_id
                # -------------------------

                if c_idx == 0xFFFFFFFF: # EOF V-SYNC
                    expected_size = f_stride * f_h * 4
                    if len(back_buffer) == expected_size and f_stride > 0 and f_h > 0:
                        
                        # Take a rapid snapshot clone for the GPU
                        with frame_lock:
                            render_buffer = bytearray(back_buffer) 
                            frame_w, frame_h, frame_stride = f_w, f_h, f_stride
                            frame_ready = True
                            
                    packets_received = 0
                    continue

                expected_size = f_stride * f_h * 4
                if len(back_buffer) != expected_size:
                    back_buffer = bytearray(expected_size)
                    w, h, stride = f_w, f_h, f_stride

                payload = data[HEADER_SIZE:]
                start = c_idx * CHUNK_SIZE
                end = start + len(payload)
                
                if end <= len(back_buffer):
                    back_buffer[start:end] = payload
                    packets_received += 1

        except Exception as e:
                pass

def main():
    global frame_ready
    
    pygame.init()
    flags = pygame.RESIZABLE | pygame.DOUBLEBUF | pygame.SCALED
    screen = pygame.display.set_mode((1920, 1080), flags)
    pygame.display.set_caption("NyxOS Live Stream & DSDT Exfiltrator")

    net_thread = threading.Thread(target=network_daemon, daemon=True)
    net_thread.start()

    running = True
    is_fullscreen = False
    clock = pygame.time.Clock()

    while running:
        for event in pygame.event.get():
            if event.type == pygame.QUIT:
                running = False
            elif event.type == pygame.KEYDOWN:
                if event.key == pygame.K_f:
                    is_fullscreen = not is_fullscreen
                    if is_fullscreen:
                        pygame.display.toggle_fullscreen()

        with frame_lock:
            if frame_ready:
                try:
                    surface = pygame.image.frombuffer(render_buffer, (frame_stride, frame_h), "BGRA")
                    surface = surface.convert()
                    
                    if frame_stride > frame_w:
                        surface = surface.subsurface((0, 0, frame_w, frame_h))
                    
                    screen.blit(surface, (0, 0))
                    pygame.display.flip()
                except Exception as e:
                    pass
                
                frame_ready = False

        clock.tick(144)

    pygame.quit()

if __name__ == "__main__":
    main()