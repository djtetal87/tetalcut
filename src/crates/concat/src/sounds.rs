// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 AJ Media Studio (TetalCut, a fork of Concat)

//! TetalCut's built-in sound bank, written by scripts/tetalcut-sounds.py:
//! every sound synthesised by that script, so free to use in any video.

/// (category, name, file name, seconds, bytes)
pub const BUILT_IN: &[(&str, &str, &str, f64, &[u8])] = &[
    ("Transiciones", "Whoosh", "transiciones/whoosh.m4a", 0.70, include_bytes!("../sounds/transiciones/whoosh.m4a")),
    ("Transiciones", "Swoosh rápido", "transiciones/swoosh-rapido.m4a", 0.35, include_bytes!("../sounds/transiciones/swoosh-rapido.m4a")),
    ("Transiciones", "Subida (riser)", "transiciones/subida-riser.m4a", 3.00, include_bytes!("../sounds/transiciones/subida-riser.m4a")),
    ("Transiciones", "Bajada (downlifter)", "transiciones/bajada-downlifter.m4a", 2.00, include_bytes!("../sounds/transiciones/bajada-downlifter.m4a")),
    ("Transiciones", "Glitch", "transiciones/glitch.m4a", 0.60, include_bytes!("../sounds/transiciones/glitch.m4a")),
    ("Impactos", "Boom", "impactos/boom.m4a", 1.60, include_bytes!("../sounds/impactos/boom.m4a")),
    ("Impactos", "Golpe", "impactos/golpe.m4a", 0.90, include_bytes!("../sounds/impactos/golpe.m4a")),
    ("Impactos", "Bass drop", "impactos/bass-drop.m4a", 2.50, include_bytes!("../sounds/impactos/bass-drop.m4a")),
    ("Impactos", "Redoble", "impactos/redoble.m4a", 2.20, include_bytes!("../sounds/impactos/redoble.m4a")),
    ("Interfaz", "Pop", "interfaz/pop.m4a", 0.12, include_bytes!("../sounds/interfaz/pop.m4a")),
    ("Interfaz", "Click", "interfaz/click.m4a", 0.05, include_bytes!("../sounds/interfaz/click.m4a")),
    ("Interfaz", "Ding", "interfaz/ding.m4a", 1.40, include_bytes!("../sounds/interfaz/ding.m4a")),
    ("Interfaz", "Notificación", "interfaz/notificacion.m4a", 0.70, include_bytes!("../sounds/interfaz/notificacion.m4a")),
    ("Interfaz", "Éxito", "interfaz/exito.m4a", 1.00, include_bytes!("../sounds/interfaz/exito.m4a")),
    ("Interfaz", "Error", "interfaz/error.m4a", 0.45, include_bytes!("../sounds/interfaz/error.m4a")),
    ("Interfaz", "Cámara", "interfaz/camara.m4a", 0.30, include_bytes!("../sounds/interfaz/camara.m4a")),
    ("Interfaz", "Teclado", "interfaz/teclado.m4a", 2.00, include_bytes!("../sounds/interfaz/teclado.m4a")),
    ("Interfaz", "Celular vibrando", "interfaz/celular-vibrando.m4a", 1.00, include_bytes!("../sounds/interfaz/celular-vibrando.m4a")),
    ("Divertidos", "Caja registradora", "divertidos/caja-registradora.m4a", 1.40, include_bytes!("../sounds/divertidos/caja-registradora.m4a")),
    ("Divertidos", "Boing", "divertidos/boing.m4a", 0.80, include_bytes!("../sounds/divertidos/boing.m4a")),
    ("Divertidos", "Scratch de disco", "divertidos/scratch-de-disco.m4a", 0.60, include_bytes!("../sounds/divertidos/scratch-de-disco.m4a")),
    ("Divertidos", "Láser", "divertidos/laser.m4a", 0.35, include_bytes!("../sounds/divertidos/laser.m4a")),
    ("Divertidos", "Magia", "divertidos/magia.m4a", 1.60, include_bytes!("../sounds/divertidos/magia.m4a")),
    ("Divertidos", "Latido", "divertidos/latido.m4a", 1.80, include_bytes!("../sounds/divertidos/latido.m4a")),
    ("Divertidos", "Tic tac", "divertidos/tic-tac.m4a", 2.00, include_bytes!("../sounds/divertidos/tic-tac.m4a")),
    ("Música", "Dembow 95", "musica/dembow-95.m4a", 30.92, include_bytes!("../sounds/musica/dembow-95.m4a")),
    ("Música", "House 124", "musica/house-124.m4a", 31.57, include_bytes!("../sounds/musica/house-124.m4a")),
    ("Música", "Trap 140", "musica/trap-140.m4a", 28.03, include_bytes!("../sounds/musica/trap-140.m4a")),
    ("Música", "Lo-fi 82", "musica/lo-fi-82.m4a", 29.87, include_bytes!("../sounds/musica/lo-fi-82.m4a")),
    ("Música", "Pop 110", "musica/pop-110.m4a", 31.15, include_bytes!("../sounds/musica/pop-110.m4a")),
    ("Música", "Corporativo 100", "musica/corporativo-100.m4a", 29.40, include_bytes!("../sounds/musica/corporativo-100.m4a")),
];
