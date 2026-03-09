// Copyright (c) 2024 Steven Rosenthal smr@dt3.org
// See LICENSE file in root directory for license terms.

use std::{path::Path, sync::Arc};

use cedar_solver::Tetra3Solver;
use pico_args::Arguments;
use tetra3::Tetra3;
use tokio::sync::Mutex;

use cedar_elements::solver_trait::SolverTrait;
use cedar_server::cedar_server::server_main;

fn main() {
    server_main(
        "Copyright (c) 2026 Steven Rosenthal smr@dt3.org.\n\
         Licensed for non-commercial use.\n\
         See LICENSE.md at https://github.com/smroid/cedar-server",
        /*flutter_app_path=*/"../cedar/cedar-aim/cedar_flutter/build/web",
        /*get_dependencies=*/
        |_pargs: Arguments| {
            let db_path = Path::new("/home/cedar/cedar/data/default_database.npz");
            let solver = Tetra3Solver::new(
                    Tetra3::load_database(db_path)
                        .expect("Failed to load Tetra3 database"),
            );
            let solver_arc: Arc<Mutex<dyn SolverTrait + Send + Sync>> = Arc::new(Mutex::new(solver));

            (None, None, None, Some(solver_arc))
        }
    );
}
