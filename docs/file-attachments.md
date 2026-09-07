# File attachments

Android 0.3.24 adds **Files** to the attachment source menu in both the API
conversation and terminal views. It uses Android's document picker, including
Downloads and document providers, without broad storage permission. Camera,
screenshot, and gallery options remain available.

Documents are copied once into a private draft file; bytes are not converted
or decoded as an image. Draft cards show the filename. Photos and documents
share the existing limits: four attachments, 12 MiB each, 48 MiB per message.
Empty/unreadable documents and oversized streams produce a visible error.
Cancellation and removal delete only private snapshots, leaving originals
intact. Successful submission clears the draft; failed submission keeps it.

The authenticated terminal upload protocol accepts an optional `file_name`
only with `application/octet-stream`. Legacy normalized JPEG declarations
keep their exact wire shape and strict JPEG validation. Documents retain
chunk ordering, digest/length verification, resumable membership, attachment
ownership checks, confined publication, and manifest-based expiry. Published
names are generated UUIDs followed by the safe original basename. Uploaded
files are referenced under `Attached files:` in the prompt.

Linux 0.10.87 and Windows 0.1.9 add a paperclip **Attach files** button in the
top toolbar and visible feedback during native file drops. Both routes paste
escaped paths into the active terminal, without sending Enter. Selecting a
file pins the destination to that session; a session change cancels insertion.
Input ownership is acquired before writing. Windows converts local drive
paths and the active WSL distribution's UNC paths into Linux paths. Other
network UNC paths are rejected with a visible explanation.

Desktop files already exist on the machine running the agent, so they are
referenced in place. Android files are uploaded to the connected desktop;
that desktop must have the matching update for document uploads.

Validation covers exact legacy and document CBOR payloads, filename handling,
mixed drafts, document prompt formatting, private copying/cancellation,
resumable publication/restart cleanup, hash mismatch rejection, and the
terminal attachment chooser. Backend upload and existing photo regressions
run alongside Android unit/device tests and shared desktop UI tests.
